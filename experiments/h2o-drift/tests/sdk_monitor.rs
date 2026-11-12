#![cfg(feature = "sdk")]

use h2o_drift_prototype::{
    drift::{Decision, Signal},
    memory::Memory,
    monitor::{
        Assessment, BranchSpec, Checkpoint, Goal, Judge, JudgeFailure, JudgementInput, SdkMonitor,
    },
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
    time::{Duration, Instant},
};

mod support;
use support::TestDir;

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
    owner: Arc<Mutex<String>>,
    fail_actions: Arc<Mutex<bool>>,
    drop_action_replies: Arc<Mutex<bool>>,
    hydration: Arc<Mutex<Vec<ApiEvent>>>,
    requests: Arc<Mutex<Vec<ApiRequest>>>,
    worker: Option<JoinHandle<()>>,
}

impl Peer {
    fn new() -> Self {
        let (to_client, client_rx) = channel();
        let (to_server, server_rx) = channel();
        let owner = Arc::new(Mutex::new("root".to_string()));
        let fail_actions = Arc::new(Mutex::new(false));
        let drop_action_replies = Arc::new(Mutex::new(false));
        let hydration = Arc::new(Mutex::new(Vec::<ApiEvent>::new()));
        let requests = Arc::new(Mutex::new(vec![]));
        let (server_owner, server_fail, server_requests, replies) = (
            owner.clone(),
            fail_actions.clone(),
            requests.clone(),
            to_client.clone(),
        );
        let server_drop_replies = drop_action_replies.clone();
        let server_hydration = hydration.clone();
        let worker = thread::spawn(move || {
            let mut reader = BufReader::new(ChannelReader {
                rx: server_rx,
                current: Cursor::new(vec![]),
            });
            while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
                let is_action = matches!(
                    &frame.request,
                    ApiRequest::Cancel { .. } | ApiRequest::SoftInterrupt { .. }
                );
                if matches!(&frame.request, ApiRequest::AttachSession { .. }) {
                    for event in server_hydration.lock().unwrap().drain(..) {
                        let mut bytes = vec![];
                        write_frame(&mut bytes, &ServerFrame::event(event)).unwrap();
                        replies.send(bytes).unwrap();
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
                            "session_id":session_id, "parent_session_id":*server_owner.lock().unwrap(), "status":"running"
                        })).unwrap();
                        ApiEvent::Attached { session: member }
                    }
                    ApiRequest::Cancel { .. } | ApiRequest::SoftInterrupt { .. } => {
                        if *server_fail.lock().unwrap() {
                            ApiEvent::Error {
                                code: jcode_sdk::api::ErrorCode::Internal,
                                message: "fixture request failure".into(),
                            }
                        } else {
                            ApiEvent::Ok
                        }
                    }
                    _ => panic!(
                        "unexpected SDK request: {}",
                        serde_json::to_string(&frame.request).unwrap()
                    ),
                };
                server_requests.lock().unwrap().push(frame.request);
                if is_action && *server_drop_replies.lock().unwrap() {
                    continue;
                }
                let mut bytes = vec![];
                write_frame(&mut bytes, &ServerFrame::reply(frame.id, reply)).unwrap();
                if replies.send(bytes).is_err() {
                    break;
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
            owner,
            fail_actions,
            drop_action_replies,
            hydration,
            requests,
            worker: Some(worker),
        }
    }

    fn emit(&self, event: ApiEvent) {
        let mut bytes = vec![];
        write_frame(&mut bytes, &ServerFrame::event(event)).unwrap();
        self.feed.send(bytes).unwrap();
    }

    fn tool(&self, call: &str, session: &str) {
        self.emit(ApiEvent::ToolDone {
            session_id: session.into(),
            call_id: call.into(),
            name: "fixture-test".into(),
            output: "observed test output".into(),
            error: None,
        });
    }

    fn attach(&self, root: &TestDir) -> SdkMonitor {
        let spec = BranchSpec {
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
            context_ids: vec!["selected".into()],
            max_corrections: 1,
        };
        let mut sources = Memory::open(root.path.join("sources")).unwrap();
        if sources.get("selected").is_err() {
            sources.insert("selected", b"context for worker a").unwrap();
        }
        SdkMonitor::attach(
            self.client.clone(),
            spec,
            sources,
            root.path.join("journal"),
        )
        .unwrap()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.feed.send(vec![]);
        let _ = self.stop.send(vec![]);
        self.worker.take().unwrap().join().unwrap();
    }
}

struct MockJudge {
    answers: VecDeque<Result<Signal, JudgeFailure>>,
    calls: usize,
}
impl Judge for MockJudge {
    fn assess(&mut self, input: &JudgementInput) -> Result<Assessment, JudgeFailure> {
        self.calls += 1;
        assert_eq!(input.context.len(), 1);
        assert_eq!(input.context[0].bytes, b"context for worker a");
        self.answers
            .pop_front()
            .expect("unexpected judge call")
            .map(|signal| Assessment {
                signal,
                reason: "scripted evidence".into(),
            })
    }
}

fn next(monitor: &mut SdkMonitor, judge: &mut MockJudge) -> Checkpoint {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(record) = monitor.poll(Duration::from_millis(10), judge).unwrap() {
            return record;
        }
        assert!(Instant::now() < deadline, "checkpoint not delivered");
    }
}

#[test]
fn real_sdk_frames_drive_replan_hold_and_branch_only_cancel() {
    let root = TestDir::new();
    let peer = Peer::new();
    let mut monitor = peer.attach(&root);
    let mut judge = MockJudge {
        answers: [
            Ok(Signal::Aligned),
            Ok(Signal::Misaligned),
            Err(JudgeFailure::Timeout),
            Ok(Signal::Misaligned),
        ]
        .into(),
        calls: 0,
    };
    peer.tool("foreign", "worker-b");
    peer.emit(ApiEvent::ReasoningDelta {
        session_id: "worker-a".into(),
        text: "excluded".into(),
    });
    for (call, expected) in [
        ("one", Decision::Continue),
        ("two", Decision::Correct),
        ("three", Decision::Hold),
        ("four", Decision::StopBranch),
    ] {
        peer.tool(call, "worker-a");
        assert_eq!(next(&mut monitor, &mut judge).decision, expected);
    }
    assert_eq!(judge.calls, 4);
    assert!(!monitor.monitor().needs_reconciliation());
    let requests = peer.requests.lock().unwrap();
    let mut replans = 0;
    let mut cancels = 0;
    for request in requests.iter() {
        match request {
            ApiRequest::SoftInterrupt {
                session_id,
                content,
                ..
            } => {
                assert_eq!(session_id, "worker-a");
                assert!(content.contains("Restore"));
                replans += 1;
            }
            ApiRequest::Cancel { session_id } => {
                assert_eq!(session_id, "worker-a");
                cancels += 1;
            }
            ApiRequest::Hello { .. } | ApiRequest::AttachSession { .. } => {}
            _ => panic!("monitor must not select models, spawn workers or delete sessions"),
        }
    }
    assert_eq!((replans, cancels), (1, 2));
    assert_eq!(
        monitor
            .monitor()
            .record(4)
            .unwrap()
            .input
            .observation
            .output,
        "observed test output"
    );
}

#[test]
fn changed_owner_prevents_control_and_keeps_the_checkpoint() {
    let root = TestDir::new();
    let peer = Peer::new();
    let mut monitor = peer.attach(&root);
    *peer.owner.lock().unwrap() = "other-root".into();
    let mut judge = MockJudge {
        answers: [Ok(Signal::Misaligned)].into(),
        calls: 0,
    };
    peer.tool("one", "worker-a");
    next(&mut monitor, &mut judge);
    assert!(monitor.monitor().needs_reconciliation());
    assert_eq!(monitor.monitor().checkpoint_count(), 1);
    assert!(peer.requests.lock().unwrap().iter().all(|r| !matches!(
        r,
        ApiRequest::SoftInterrupt { .. } | ApiRequest::Cancel { .. }
    )));
}

#[test]
fn sdk_control_failure_is_not_retried_after_reopen() {
    let root = TestDir::new();
    let peer = Peer::new();
    *peer.fail_actions.lock().unwrap() = true;
    let mut judge = MockJudge {
        answers: [Ok(Signal::Misaligned)].into(),
        calls: 0,
    };
    let mut monitor = peer.attach(&root);
    peer.tool("one", "worker-a");
    next(&mut monitor, &mut judge);
    assert!(monitor.monitor().needs_reconciliation());
    drop(monitor);
    let reopened = peer.attach(&root);
    assert!(reopened.monitor().needs_reconciliation());
    assert_eq!(
        peer.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| matches!(r, ApiRequest::SoftInterrupt { .. }))
            .count(),
        1
    );
}

#[test]
fn idle_timeout_is_not_failure_but_disconnection_is_unknown_and_uncertain() {
    let root = TestDir::new();
    let peer = Peer::new();
    let mut monitor = peer.attach(&root);
    let mut judge = MockJudge {
        answers: [].into(),
        calls: 0,
    };
    for _ in 0..3 {
        assert!(
            monitor
                .poll(Duration::from_millis(1), &mut judge)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(monitor.monitor().checkpoint_count(), 0);
    peer.feed.send(vec![]).unwrap();
    let record = next(&mut monitor, &mut judge);
    assert_eq!(record.assessment.signal, Signal::Unknown);
    assert_eq!(record.decision, Decision::Hold);
    assert!(monitor.monitor().needs_reconciliation());
    assert_eq!(judge.calls, 0);
}

#[test]
fn tools_hydrated_before_attach_reply_are_not_lost() {
    let root = TestDir::new();
    let peer = Peer::new();
    peer.hydration.lock().unwrap().push(ApiEvent::ToolDone {
        session_id: "worker-a".into(),
        call_id: "hydrated".into(),
        name: "fixture-test".into(),
        output: "observed before attach completed".into(),
        error: None,
    });
    let mut monitor = peer.attach(&root);
    let mut judge = MockJudge {
        answers: [Ok(Signal::Aligned)].into(),
        calls: 0,
    };
    assert_eq!(
        next(&mut monitor, &mut judge).input.observation.call_id,
        "hydrated"
    );
    assert_eq!(judge.calls, 1);
}

#[test]
fn lost_control_acknowledgement_times_out_without_automatic_retransmission() {
    let root = TestDir::new();
    let peer = Peer::new();
    *peer.drop_action_replies.lock().unwrap() = true;
    let mut monitor = peer.attach(&root);
    let mut judge = MockJudge {
        answers: [Ok(Signal::Misaligned)].into(),
        calls: 0,
    };
    peer.tool("one", "worker-a");
    assert_eq!(next(&mut monitor, &mut judge).decision, Decision::Correct);
    assert!(monitor.monitor().needs_reconciliation());
    drop(monitor);
    let reopened = peer.attach(&root);
    assert!(reopened.monitor().needs_reconciliation());
    assert_eq!(
        peer.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| matches!(r, ApiRequest::SoftInterrupt { .. }))
            .count(),
        1
    );
}
