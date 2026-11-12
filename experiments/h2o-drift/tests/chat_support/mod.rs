//! Scripted jcode harness peer over an in-memory transport, driving a REAL `JcodeClient`.
//! Never touches a real endpoint, daemon, provider, key or network.
#![allow(dead_code)]

use h2o_drift_prototype::chat_transport::{ChatController, ChatEvent, ConnectConfig};
use jcode_sdk::{
    ApiEvent, ApiRequest, SessionInfo, SessionToolDefinition, Transport,
    api::{API_VERSION_MAJOR, ClientFrame, ErrorCode, ServerFrame, read_frame, write_frame},
};
use std::{
    io::{self, BufRead, BufReader, Cursor, Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};

pub enum Reply {
    Ok,
    Error,
    Silent,
}

#[allow(clippy::large_enum_variant)]
pub enum Step {
    Event(ApiEvent),
    Sleep(Duration),
    /// Close the connection from the peer side.
    Disconnect,
}

pub type TurnScript = Arc<dyn Fn(&str, &str) -> Vec<Step> + Send + Sync>;

pub struct Script {
    pub capabilities: Vec<String>,
    pub hello: Reply,
    pub configure: Reply,
    pub effective_tools: Vec<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// On Cancel: reply Ok and end the turn (TurnStopped + TurnDone).
    pub cancel_ends_turn: bool,
    /// On Cancel: reply Ok but let the turn script keep running (overrides `cancel_ends_turn`).
    pub cancel_ack_only: bool,
    /// Session id created on connection `n` (default `sess-n`).
    pub session_id_for: fn(usize) -> String,
    /// Model reported on connection `n`; `None` = always `model`.
    pub model_for: Option<fn(usize) -> Option<String>>,
    pub turn: TurnScript,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            capabilities: [
                "sessions",
                "streaming",
                "text_framing",
                "turn_stop_reasons",
                "runtime_info",
                "session_tools",
            ]
            .map(String::from)
            .to_vec(),
            hello: Reply::Ok,
            configure: Reply::Ok,
            effective_tools: vec![],
            provider: Some("scripted-provider".into()),
            model: Some("scripted-model".into()),
            cancel_ends_turn: true,
            cancel_ack_only: false,
            session_id_for: |conn| format!("sess-{conn}"),
            model_for: None,
            turn: Arc::new(|session, _| reply_steps(session, "scripted reply")),
        }
    }
}

pub fn ev(event: ApiEvent) -> Step {
    Step::Event(event)
}

pub fn delta(session: &str, id: Option<&str>, text: &str) -> Step {
    ev(ApiEvent::TextDelta {
        session_id: session.into(),
        text: text.into(),
        message_id: id.map(String::from),
    })
}
pub fn done(session: &str, id: Option<&str>) -> Step {
    ev(ApiEvent::TextDone {
        session_id: session.into(),
        message_id: id.map(String::from),
    })
}
pub fn replace(session: &str, id: Option<&str>, text: &str) -> Step {
    ev(ApiEvent::TextReplace {
        session_id: session.into(),
        text: text.into(),
        message_id: id.map(String::from),
    })
}
pub fn turn_done(session: &str) -> Step {
    ev(ApiEvent::TurnDone {
        session_id: session.into(),
        pending_soft_interrupts: None,
    })
}

/// A framed, accepted, completed single-message reply.
pub fn reply_steps(session: &str, text: &str) -> Vec<Step> {
    vec![
        ev(ApiEvent::MessageAccepted {
            session_id: session.into(),
        }),
        delta(session, Some("m1"), text),
        done(session, Some("m1")),
        turn_done(session),
    ]
}

// --- in-memory duplex transport (same shape as tests/sdk_monitor.rs) ---

struct ChannelReader {
    rx: Receiver<Vec<u8>>,
    current: Cursor<Vec<u8>>,
}
impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.current.read(out)?;
            if count != 0 {
                return Ok(count);
            }
            match self.rx.recv() {
                Ok(bytes) if !bytes.is_empty() => self.current = Cursor::new(bytes),
                _ => return Ok(0), // empty chunk or closed = EOF
            }
        }
    }
}

struct ChannelWriter(Sender<Vec<u8>>);
impl Write for ChannelWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(bytes.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct LocalTransport {
    rx: Receiver<Vec<u8>>,
    tx: Sender<Vec<u8>>,
    close: Sender<Vec<u8>>,
}
impl Transport for LocalTransport {
    fn shutdown_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let close = self.close.clone();
        Some(Arc::new(move || {
            let _ = close.send(vec![]);
        }))
    }
    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        Ok((
            Box::new(BufReader::new(ChannelReader {
                rx: self.rx,
                current: Cursor::new(vec![]),
            })),
            Box::new(ChannelWriter(self.tx)),
        ))
    }
}

// --- harness ---

struct Shared {
    script: Script,
    /// (connection index, request) in arrival order.
    log: Mutex<Vec<(usize, ApiRequest)>>,
    endpoints: Mutex<Vec<PathBuf>>,
    to_client: Mutex<Vec<Sender<Vec<u8>>>>,
    /// Peer-side "connection closed" flags; the peer stops writing once set.
    closed: Mutex<Vec<Arc<AtomicBool>>>,
}

#[derive(Clone)]
pub struct Harness(Arc<Shared>);

pub const TEST_ENDPOINT: &str = "h2o-test-endpoint";

impl Harness {
    pub fn new(script: Script) -> Self {
        Self(Arc::new(Shared {
            script,
            log: Mutex::new(vec![]),
            endpoints: Mutex::new(vec![]),
            to_client: Mutex::new(vec![]),
            closed: Mutex::new(vec![]),
        }))
    }

    pub fn config(&self) -> ConnectConfig {
        ConnectConfig {
            endpoint: TEST_ENDPOINT.into(),
            request_timeout: Duration::from_secs(2),
            turn_timeout: Duration::from_secs(5),
        }
    }

    /// A REAL `ChatController` whose transport factory connects to this peer.
    pub fn controller(&self) -> ChatController {
        let shared = self.0.clone();
        ChatController::with_transport_factory(move |path| {
            let (to_client, client_rx) = channel::<Vec<u8>>();
            let (to_server, server_rx) = channel::<Vec<u8>>();
            let conn = {
                let mut endpoints = shared.endpoints.lock().unwrap();
                endpoints.push(path.to_path_buf());
                endpoints.len() - 1
            };
            let closed = Arc::new(AtomicBool::new(false));
            shared.to_client.lock().unwrap().push(to_client.clone());
            shared.closed.lock().unwrap().push(closed.clone());
            let peer = shared.clone();
            let out = to_client.clone();
            thread::spawn(move || serve(peer, conn, server_rx, out, closed));
            Ok(Box::new(LocalTransport {
                rx: client_rx,
                tx: to_server,
                close: to_client,
            }) as Box<dyn Transport>)
        })
    }

    pub fn requests(&self) -> Vec<ApiRequest> {
        self.0
            .log
            .lock()
            .unwrap()
            .iter()
            .map(|(_, r)| r.clone())
            .collect()
    }
    pub fn requests_on(&self, conn: usize) -> Vec<ApiRequest> {
        self.0
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| *c == conn)
            .map(|(_, r)| r.clone())
            .collect()
    }
    pub fn endpoints(&self) -> Vec<PathBuf> {
        self.0.endpoints.lock().unwrap().clone()
    }
    pub fn drop_connection(&self, conn: usize) {
        close(&self.0, conn);
    }
    pub fn count(&self, pred: impl Fn(&ApiRequest) -> bool) -> usize {
        self.requests().iter().filter(|r| pred(r)).count()
    }
}

fn close(shared: &Shared, conn: usize) {
    shared.closed.lock().unwrap()[conn].store(true, Ordering::SeqCst);
    let _ = shared.to_client.lock().unwrap()[conn].send(vec![]);
}

fn send(out: &Sender<Vec<u8>>, closed: &AtomicBool, frame: ServerFrame) {
    if closed.load(Ordering::SeqCst) {
        return;
    }
    let mut bytes = vec![];
    write_frame(&mut bytes, &frame).unwrap();
    let _ = out.send(bytes);
}

fn serve(
    shared: Arc<Shared>,
    conn: usize,
    rx: Receiver<Vec<u8>>,
    out: Sender<Vec<u8>>,
    closed: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(ChannelReader {
        rx,
        current: Cursor::new(vec![]),
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
        let ClientFrame { id, request, .. } = frame;
        shared.log.lock().unwrap().push((conn, request.clone()));
        let s = &shared.script;
        let sid = (s.session_id_for)(conn);
        let reply = |event| send(&out, &closed, ServerFrame::reply(id, event));
        let error = |message: &str| ApiEvent::Error {
            code: ErrorCode::InvalidRequest,
            message: message.into(),
        };
        match request {
            ApiRequest::Hello { .. } => match s.hello {
                Reply::Ok => reply(ApiEvent::HelloOk {
                    version: API_VERSION_MAJOR,
                    server: "scripted-harness".into(),
                    capabilities: s.capabilities.clone(),
                }),
                Reply::Error => reply(error("hello refused by fixture")),
                Reply::Silent => {}
            },
            ApiRequest::CreateSession { .. } => {
                let session: SessionInfo = serde_json::from_value(
                    serde_json::json!({"session_id": sid, "status": "idle"}),
                )
                .unwrap();
                reply(ApiEvent::Attached { session });
            }
            ApiRequest::ConfigureTools { .. } => match s.configure {
                Reply::Ok => reply(ApiEvent::Ok),
                Reply::Error => reply(error("fixture configure_tools failure")),
                Reply::Silent => {}
            },
            ApiRequest::ListTools { .. } => reply(ApiEvent::Tools {
                session_id: sid.clone(),
                tools: s
                    .effective_tools
                    .iter()
                    .map(|name| SessionToolDefinition {
                        name: name.clone(),
                        description: "fixture".into(),
                        parameters: serde_json::Map::new(),
                    })
                    .collect(),
            }),
            ApiRequest::Ping => reply(ApiEvent::Pong),
            ApiRequest::GetRuntimeInfo { .. } => reply(ApiEvent::RuntimeInfo {
                session_id: sid.clone(),
                provider: s.provider.clone(),
                model: s.model_for.map_or_else(|| s.model.clone(), |f| f(conn)),
                reasoning_effort: None,
                auth_method: None,
                account_label: None,
                routes: vec![],
            }),
            ApiRequest::SendMessage {
                session_id,
                content,
                ..
            } => {
                cancelled.store(false, Ordering::SeqCst);
                let steps = (s.turn)(&session_id, &content);
                let (peer, out, closed, cancelled) = (
                    shared.clone(),
                    out.clone(),
                    closed.clone(),
                    cancelled.clone(),
                );
                thread::spawn(move || {
                    for step in steps {
                        if cancelled.load(Ordering::SeqCst) {
                            return;
                        }
                        match step {
                            Step::Event(event) => send(&out, &closed, ServerFrame::event(event)),
                            Step::Sleep(d) => thread::sleep(d),
                            Step::Disconnect => close(&peer, conn),
                        }
                    }
                });
            }
            ApiRequest::Cancel { session_id } => {
                if s.cancel_ack_only {
                    reply(ApiEvent::Ok);
                } else if s.cancel_ends_turn {
                    cancelled.store(true, Ordering::SeqCst);
                    reply(ApiEvent::Ok);
                    send(
                        &out,
                        &closed,
                        ServerFrame::event(ApiEvent::TurnStopped {
                            session_id: session_id.clone(),
                            reason: jcode_sdk::TurnStopReason::Interrupted,
                            message: "cancelled by client".into(),
                            provider_stop_reason: None,
                        }),
                    );
                    send(
                        &out,
                        &closed,
                        ServerFrame::event(ApiEvent::TurnDone {
                            session_id,
                            pending_soft_interrupts: None,
                        }),
                    );
                }
            }
            ApiRequest::PermissionResponse { .. } => reply(ApiEvent::Ok),
            _ => reply(error("request not expected by chat fixture")),
        }
    }
}

// --- polling helpers ---

/// Poll `try_recv` until `pred` matches an event (inclusive) or `timeout`.
pub fn wait_for(
    ctrl: &mut ChatController,
    timeout: Duration,
    pred: impl Fn(&ChatEvent) -> bool,
) -> Vec<ChatEvent> {
    let deadline = Instant::now() + timeout;
    let mut seen = vec![];
    while Instant::now() < deadline {
        match ctrl.try_recv() {
            Some(event) => {
                let hit = pred(&event);
                seen.push(event);
                if hit {
                    return seen;
                }
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("timed out after {timeout:?} waiting for event; seen: {seen:#?}");
}

/// Collect every event for `period`.
pub fn drain(ctrl: &mut ChatController, period: Duration) -> Vec<ChatEvent> {
    let deadline = Instant::now() + period;
    let mut seen = vec![];
    while Instant::now() < deadline {
        match ctrl.try_recv() {
            Some(event) => seen.push(event),
            None => thread::sleep(Duration::from_millis(5)),
        }
    }
    seen
}

pub fn is_ready(e: &ChatEvent) -> bool {
    matches!(e, ChatEvent::Ready { .. })
}
pub fn is_terminal(e: &ChatEvent) -> bool {
    matches!(e, ChatEvent::Reply { .. } | ChatEvent::Failed { .. })
}
pub fn is_disconnected(e: &ChatEvent) -> bool {
    matches!(e, ChatEvent::Disconnected { .. })
}

/// Connect and wait for Ready; returns the events seen.
pub fn connect_ready(
    harness: &Harness,
    ctrl: &mut ChatController,
    conversation_id: u64,
) -> Vec<ChatEvent> {
    ctrl.connect(harness.config(), conversation_id).unwrap();
    wait_for(ctrl, Duration::from_secs(5), is_ready)
}
