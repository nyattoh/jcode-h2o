#![cfg(feature = "sdk")]
//! `run` against an in-memory scripted SDK peer. No daemon, no network beyond 127.0.0.1.

use h2o_drift_prototype::{
    drift::Signal,
    memory::Memory,
    monitor::{Assessment, BranchSpec, Goal, Judge, JudgeFailure, JudgementInput},
    run::{RunEnd, run},
};
use jcode_sdk::{
    ApiEvent, ApiRequest, ConnectOptions, JcodeClient, SessionInfo, Transport,
    api::{API_VERSION_MAJOR, ClientFrame, ServerFrame, read_frame, write_frame},
};
use std::{
    collections::VecDeque,
    io::{self, BufRead, BufReader, Cursor, Read, Write},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

mod support;
use support::TestDir;

// Minimal copy of the scripted peer in tests/sdk_monitor.rs.
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
                _ => return Ok(0),
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

struct Peer {
    client: JcodeClient,
    feed: Sender<Vec<u8>>,
    stop: Sender<Vec<u8>>,
    /// Events delivered when the first AttachSession arrives, so `run` cannot miss them.
    on_attach: Arc<Mutex<Vec<ApiEvent>>>,
    fail_actions: Arc<Mutex<bool>>,
    /// Close the transport right after replying to the first AttachSession (no wall clock).
    close_on_attach: Arc<Mutex<bool>>,
    requests: Arc<Mutex<Vec<ApiRequest>>>,
    worker: Option<JoinHandle<()>>,
}

impl Peer {
    fn new() -> Self {
        let (to_client, client_rx) = channel();
        let (to_server, server_rx) = channel();
        let on_attach = Arc::new(Mutex::new(Vec::<ApiEvent>::new()));
        let fail_actions = Arc::new(Mutex::new(false));
        let close_on_attach = Arc::new(Mutex::new(false));
        let server_close = close_on_attach.clone();
        let requests = Arc::new(Mutex::new(vec![]));
        let (server_attach, server_fail, server_requests, replies) = (
            on_attach.clone(),
            fail_actions.clone(),
            requests.clone(),
            to_client.clone(),
        );
        let worker = thread::spawn(move || {
            let mut reader = BufReader::new(ChannelReader {
                rx: server_rx,
                current: Cursor::new(vec![]),
            });
            while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
                if matches!(&frame.request, ApiRequest::AttachSession { .. }) {
                    for event in server_attach.lock().unwrap().drain(..) {
                        let mut bytes = vec![];
                        write_frame(&mut bytes, &ServerFrame::event(event)).unwrap();
                        let _ = replies.send(bytes);
                    }
                }
                let reply = match &frame.request {
                    ApiRequest::Hello { .. } => ApiEvent::HelloOk {
                        version: API_VERSION_MAJOR,
                        server: "offline-scripted-peer".into(),
                        capabilities: vec![],
                    },
                    ApiRequest::AttachSession { session_id } => {
                        let member: SessionInfo = serde_json::from_value(serde_json::json!({
                            "session_id": session_id, "parent_session_id": "root", "status": "running"
                        }))
                        .unwrap();
                        ApiEvent::Attached { session: member }
                    }
                    ApiRequest::Cancel { .. } | ApiRequest::SoftInterrupt { .. }
                        if *server_fail.lock().unwrap() =>
                    {
                        ApiEvent::Error {
                            code: jcode_sdk::api::ErrorCode::Internal,
                            message: "fixture request failure".into(),
                        }
                    }
                    ApiRequest::Cancel { .. } | ApiRequest::SoftInterrupt { .. } => ApiEvent::Ok,
                    // Recorded and refused, so the test can fail on it instead of the peer panicking.
                    _ => ApiEvent::Error {
                        code: jcode_sdk::api::ErrorCode::Internal,
                        message: "unexpected request".into(),
                    },
                };
                let attach = matches!(&frame.request, ApiRequest::AttachSession { .. });
                server_requests.lock().unwrap().push(frame.request);
                let mut bytes = vec![];
                write_frame(&mut bytes, &ServerFrame::reply(frame.id, reply)).unwrap();
                if replies.send(bytes).is_err() {
                    break;
                }
                if attach && std::mem::take(&mut *server_close.lock().unwrap()) {
                    let _ = replies.send(vec![]);
                }
            }
        });
        let client = JcodeClient::connect_with(
            Box::new(LocalTransport {
                rx: client_rx,
                tx: to_server.clone(),
                close: to_client.clone(),
            }),
            ConnectOptions {
                ensure_runtime: false,
                request_timeout: Some(Duration::from_secs(2)),
                ..ConnectOptions::default()
            },
        )
        .unwrap();
        Self {
            client,
            feed: to_client,
            stop: to_server,
            on_attach,
            fail_actions,
            close_on_attach,
            requests,
            worker: Some(worker),
        }
    }

    fn tool_on_attach(&self, call: &str, session: &str) {
        self.on_attach.lock().unwrap().push(ApiEvent::ToolDone {
            session_id: session.into(),
            call_id: call.into(),
            name: "fixture-test".into(),
            output: "observed test output".into(),
            error: None,
        });
    }

    fn close_on_attach(&self) {
        *self.close_on_attach.lock().unwrap() = true;
    }

    fn count(&self, pick: fn(&ApiRequest) -> bool) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| pick(r))
            .count()
    }

    fn only_expected_requests(&self) {
        for request in self.requests.lock().unwrap().iter() {
            match request {
                ApiRequest::Hello { .. }
                | ApiRequest::AttachSession { .. }
                | ApiRequest::Cancel { .. }
                | ApiRequest::SoftInterrupt { .. } => {}
                other => panic!(
                    "unexpected SDK request: {}",
                    serde_json::to_string(other).unwrap()
                ),
            }
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.feed.send(vec![]);
        let _ = self.stop.send(vec![]);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn spec(max_corrections: u32) -> BranchSpec {
    BranchSpec {
        run_id: "run-1".into(),
        branch_id: "branch-a".into(),
        owner_session_id: "root".into(),
        session_id: "worker-a".into(),
        goal: Goal {
            purpose: "Verify source recovery".into(),
            constraints: vec!["No network".into()],
            success_conditions: vec!["Byte equality".into()],
            owned_paths: vec!["experiment".into()],
            unfinished: vec!["Restore source".into()],
        },
        context_ids: vec![],
        max_corrections,
    }
}

struct MockJudge {
    answers: VecDeque<Result<Signal, JudgeFailure>>,
    calls: usize,
}
impl MockJudge {
    fn new(answers: impl Into<VecDeque<Result<Signal, JudgeFailure>>>) -> Self {
        Self {
            answers: answers.into(),
            calls: 0,
        }
    }
}
impl Judge for MockJudge {
    fn assess(&mut self, input: &JudgementInput) -> Result<Assessment, JudgeFailure> {
        self.calls += 1;
        assert!(input.observation.call_id.starts_with("own-"));
        self.answers
            .pop_front()
            .expect("unexpected judge call")
            .map(|signal| Assessment {
                signal,
                reason: "scripted evidence".into(),
            })
    }
}

fn run_with(
    peer: &Peer,
    root: &TestDir,
    spec: BranchSpec,
    judge: &mut impl Judge,
) -> (RunEnd, u64) {
    let sources = Memory::open(root.path.join("sources")).unwrap();
    run(
        peer.client.clone(),
        spec,
        sources,
        &root.path.join("journal"),
        judge,
        Duration::from_millis(10),
    )
    .unwrap()
}

fn is_cancel(r: &ApiRequest) -> bool {
    matches!(r, ApiRequest::Cancel { session_id } if session_id == "worker-a")
}
fn is_replan(r: &ApiRequest) -> bool {
    matches!(r, ApiRequest::SoftInterrupt { session_id, .. } if session_id == "worker-a")
}

#[test]
fn misaligned_beyond_budget_stops_and_cancels() {
    let root = TestDir::new();
    let peer = Peer::new();
    peer.tool_on_attach("own-1", "worker-a");
    peer.tool_on_attach("own-2", "worker-a");
    let mut judge = MockJudge::new([Ok(Signal::Misaligned), Ok(Signal::Misaligned)]);
    assert_eq!(
        run_with(&peer, &root, spec(1), &mut judge),
        (RunEnd::Stopped, 2)
    );
    assert_eq!(judge.calls, 2);
    assert_eq!(peer.count(is_replan), 1);
    assert_eq!(peer.count(is_cancel), 1);
    peer.only_expected_requests();
}

#[test]
fn closed_transport_needs_reconciliation_after_one_unknown_checkpoint() {
    let root = TestDir::new();
    let peer = Peer::new();
    peer.close_on_attach();
    let mut judge = MockJudge::new([]);
    // The Hold's cancel cannot reach a closed client, so this is not a clean finish.
    assert_eq!(
        run_with(&peer, &root, spec(1), &mut judge),
        (RunEnd::NeedsReconciliation, 1)
    );
    assert_eq!(judge.calls, 0);
    // Reopening the journal shows exactly one transport-closed record.
    let journal = Memory::open(root.path.join("journal")).unwrap();
    let record: h2o_drift_prototype::monitor::Checkpoint =
        serde_json::from_slice(&journal.get("event-00000000000000000001").unwrap()).unwrap();
    assert_eq!(record.input.observation.tool, "transport");
    assert_eq!(record.assessment.signal, Signal::Unknown);
    assert!(journal.get("event-00000000000000000002").is_err());
}

#[test]
fn failed_control_request_needs_reconciliation() {
    for (max_corrections, pick) in [(1, is_replan as fn(&ApiRequest) -> bool), (0, is_cancel)] {
        let root = TestDir::new();
        let peer = Peer::new();
        *peer.fail_actions.lock().unwrap() = true;
        peer.tool_on_attach("own-1", "worker-a");
        peer.tool_on_attach("own-2", "worker-a");
        let mut judge = MockJudge::new([Ok(Signal::Misaligned)]);
        assert_eq!(
            run_with(&peer, &root, spec(max_corrections), &mut judge),
            (RunEnd::NeedsReconciliation, 1)
        );
        assert_eq!(judge.calls, 1);
        assert_eq!(peer.count(pick), 1);
        peer.only_expected_requests();
    }
}

#[test]
fn events_for_other_sessions_are_ignored() {
    let root = TestDir::new();
    let peer = Peer::new();
    peer.tool_on_attach("foreign-1", "worker-b");
    peer.tool_on_attach("foreign-2", "root");
    peer.tool_on_attach("own-1", "worker-a");
    let mut judge = MockJudge::new([Ok(Signal::Misaligned)]);
    assert_eq!(
        run_with(&peer, &root, spec(0), &mut judge),
        (RunEnd::Stopped, 1)
    );
    assert_eq!(judge.calls, 1);
}

#[test]
fn resumed_stopped_journal_returns_stopped_without_new_requests() {
    let root = TestDir::new();
    {
        let peer = Peer::new();
        peer.tool_on_attach("own-1", "worker-a");
        let mut judge = MockJudge::new([Ok(Signal::Misaligned)]);
        assert_eq!(
            run_with(&peer, &root, spec(0), &mut judge),
            (RunEnd::Stopped, 1)
        );
    }
    let peer = Peer::new();
    let mut judge = MockJudge::new([]);
    assert_eq!(
        run_with(&peer, &root, spec(0), &mut judge),
        (RunEnd::Stopped, 1)
    );
    assert_eq!(peer.count(is_cancel), 0);
}

#[cfg(feature = "jev")]
#[test]
fn jev_judge_drives_run_over_loopback() {
    use h2o_drift_prototype::jev::{JevConfig, JevJudge, QUESTION_ID, UreqTransport};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let key = format!("test-key-run-{}", std::process::id());
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        let length: usize = head
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let reply = serde_json::to_vec(&serde_json::json!({
            "model": "jev-2026-10-01",
            "answers": {QUESTION_ID: {"type": "choice", "choice": "misaligned",
                "probabilities": {"aligned": 0.05, "misaligned": 0.9, "insufficient_evidence": 0.05}}}
        }))
        .unwrap();
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reply.len()
        )
        .unwrap();
        stream.write_all(&reply).unwrap();
        head
    });
    let env_key = key.clone();
    let mut judge = JevJudge::with_parts(
        JevConfig {
            enabled: true,
            base_url: base,
            deadline: Duration::from_secs(5),
            ..JevConfig::default()
        },
        Box::new(UreqTransport),
        Box::new(move |name| (name == "TYPESAFE_API_KEY").then(|| env_key.clone())),
    )
    .unwrap();
    let root = TestDir::new();
    let peer = Peer::new();
    peer.tool_on_attach("own-1", "worker-a");
    assert_eq!(
        run_with(&peer, &root, spec(0), &mut judge),
        (RunEnd::Stopped, 1)
    );
    let head = server.join().unwrap();
    assert!(head.contains(&format!("Bearer {key}")));
    assert_eq!(peer.count(is_cancel), 1);
    let journal = Memory::open(root.path.join("journal")).unwrap();
    let record = journal.get("event-00000000000000000001").unwrap();
    assert!(String::from_utf8_lossy(&record).contains("jev misaligned"));
    assert!(!String::from_utf8_lossy(&record).contains(&key));
}
