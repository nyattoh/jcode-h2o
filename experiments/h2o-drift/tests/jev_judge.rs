#![cfg(feature = "jev")]
//! JevJudge against a loopback-only HTTP server. Never reads the real TYPESAFE_API_KEY.

use h2o_drift_prototype::{
    drift::Signal,
    jev::{EnvLookup, JevConfig, JevJudge, QUESTION_ID, UreqTransport},
    memory::Memory,
    monitor::{
        Action, BranchControl, BranchSpec, Goal, Judge, JudgeFailure, JudgementInput, Monitor,
        Observation, Source,
    },
};
use jcode_sdk::{ApiEvent, SessionInfo};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
    sync::mpsc::{Receiver, channel},
    thread,
    time::{Duration, Instant, SystemTime},
};

mod support;
use support::TestDir;

fn fake_key() -> String {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-key-{}-{nonce}", std::process::id())
}

fn env_with(key: Option<String>) -> EnvLookup {
    Box::new(move |name| {
        assert_eq!(name, "TYPESAFE_API_KEY");
        key.clone()
    })
}

struct Request {
    head: String,
    body: Vec<u8>,
}

enum Reply {
    Respond(u16, Vec<u8>),
    Stall(Duration),
}

/// Serves exactly one request on 127.0.0.1 and reports what it received.
fn serve(reply: Reply) -> (String, Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = channel();
    thread::spawn(move || {
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
        let length = head
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let _ = tx.send(Request { head, body });
        let mut stream = stream;
        match reply {
            Reply::Respond(status, body) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
            Reply::Stall(duration) => thread::sleep(duration),
        }
    });
    (base, rx)
}

fn config(base: &str) -> JevConfig {
    JevConfig {
        enabled: true,
        base_url: base.into(),
        deadline: Duration::from_secs(5),
        min_probability: 0.6,
        ..JevConfig::default()
    }
}

fn judge(base: &str, key: &str) -> JevJudge {
    JevJudge::with_parts(
        config(base),
        Box::new(UreqTransport),
        env_with(Some(key.into())),
    )
    .unwrap()
}

fn goal() -> Goal {
    Goal {
        purpose: "Ship the parser fix".into(),
        constraints: vec!["Touch only src/parser.rs".into()],
        success_conditions: vec!["Parser tests pass".into()],
        owned_paths: vec!["src/parser.rs".into()],
        unfinished: vec!["Run regression".into()],
    }
}

const CONTEXT_BYTES: &[u8] = b"SECRET-CONTEXT-MARKER original source";

fn input(output: &str) -> JudgementInput {
    JudgementInput {
        goal: goal(),
        observation: Observation {
            call_id: "call-marker-77".into(),
            tool: "bash".into(),
            output: output.into(),
            error: Some("warning: unused".into()),
        },
        context: vec![Source {
            id: "source-a".into(),
            bytes: CONTEXT_BYTES.to_vec(),
        }],
    }
}

fn answer(choice: &str, probabilities: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": "jev-2026-10-01",
        "answers": {QUESTION_ID: {"type": "choice", "choice": choice, "probabilities": probabilities}}
    }))
    .unwrap()
}

fn probs(aligned: f64, misaligned: f64, insufficient: f64) -> Value {
    json!({"aligned": aligned, "misaligned": misaligned, "insufficient_evidence": insufficient})
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn labels_map_to_signals_and_request_has_expected_shape() {
    for (label, p, signal) in [
        ("aligned", probs(0.9, 0.05, 0.05), Signal::Aligned),
        ("misaligned", probs(0.1, 0.8, 0.1), Signal::Misaligned),
        (
            "insufficient_evidence",
            probs(0.1, 0.2, 0.7),
            Signal::Unknown,
        ),
    ] {
        let key = fake_key();
        let (base, rx) = serve(Reply::Respond(200, answer(label, p)));
        let mut judge = judge(&base, &key);
        let assessment = judge.assess(&input("cargo test: ok")).unwrap();
        assert_eq!(assessment.signal, signal, "{label}");
        assert!(assessment.reason.contains(label));
        assert!(assessment.reason.contains("jev-2026-10-01"));
        assert!(!assessment.reason.contains(&key));

        let request = rx.recv().unwrap();
        assert!(request.head.starts_with("POST /v1/systemone HTTP/1.1\r\n"));
        assert!(
            request
                .head
                .lines()
                .any(|l| l.eq_ignore_ascii_case(&format!("authorization: Bearer {key}")))
        );
        assert!(!contains(&request.body, &key));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["model"], "jev-latest");
        let question = &body["questions"][QUESTION_ID];
        assert_eq!(question["type"], "choice");
        let mut labels: Vec<_> = question["criteria"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        labels.sort();
        assert_eq!(labels, ["aligned", "insufficient_evidence", "misaligned"]);
        let state = &body["state"];
        assert_eq!(state["goal"]["purpose"], "Ship the parser fix");
        assert_eq!(state["goal"]["owned_paths"][0], "src/parser.rs");
        assert_eq!(state["observation"]["tool"], "bash");
        assert_eq!(state["observation"]["output"]["text"], "cargo test: ok");
        assert_eq!(state["observation"]["error"]["text"], "warning: unused");
        assert!(!contains(&request.body, "call-marker-77"));
        assert_eq!(judge.last_request_body(), Some(request.body.as_slice()));
    }
}

#[test]
fn low_top_probability_is_unknown() {
    let (base, _rx) = serve(Reply::Respond(200, answer("aligned", probs(0.5, 0.3, 0.2))));
    let assessment = judge(&base, &fake_key()).assess(&input("ok")).unwrap();
    assert_eq!(assessment.signal, Signal::Unknown);
    assert!(assessment.reason.contains("aligned"));
}

#[test]
fn http_500_is_unavailable() {
    let (base, _rx) = serve(Reply::Respond(500, b"{\"error\":\"boom\"}".to_vec()));
    assert!(matches!(
        judge(&base, &fake_key()).assess(&input("ok")),
        Err(JudgeFailure::Unavailable)
    ));
}

#[test]
fn non_json_body_is_malformed() {
    let (base, _rx) = serve(Reply::Respond(200, b"<html>not json</html>".to_vec()));
    assert!(matches!(
        judge(&base, &fake_key()).assess(&input("ok")),
        Err(JudgeFailure::Malformed)
    ));
}

#[test]
fn unexpected_shapes_are_malformed() {
    let q = QUESTION_ID;
    let variants = [
        json!([1, 2, 3]),
        json!({"answers": {}}),
        json!({"answers": {q: "aligned"}}),
        json!({"answers": {q: {"type": "noul", "choice": "aligned", "probabilities": probs(0.9, 0.05, 0.05)}}}),
        json!({"answers": {q: {"type": "choice", "choice": "maybe", "probabilities": {"maybe": 0.9}}}}),
        json!({"answers": {q: {"type": "choice", "choice": "aligned", "probabilities": probs(0.3, 0.6, 0.1)}}}),
        json!({"answers": {q: {"type": "choice", "choice": "aligned", "probabilities": {"aligned": 0.9, "other": 0.1}}}}),
        json!({"answers": {q: {"type": "choice", "choice": "aligned", "probabilities": {"aligned": 1.5}}}}),
        json!({"answers": {q: {"type": "choice", "choice": "aligned", "probabilities": {"aligned": "0.9"}}}}),
        json!({"answers": {q: {"type": "choice", "choice": "aligned"}}}),
        json!({"model": 7, "answers": {q: {"type": "choice", "choice": "aligned", "probabilities": probs(0.9, 0.05, 0.05)}}}),
    ];
    for variant in variants {
        let (base, _rx) = serve(Reply::Respond(200, serde_json::to_vec(&variant).unwrap()));
        assert!(
            matches!(
                judge(&base, &fake_key()).assess(&input("ok")),
                Err(JudgeFailure::Malformed)
            ),
            "{variant}"
        );
    }
}

#[test]
fn probabilities_need_every_label_exactly_once() {
    let q = QUESTION_ID;
    let bodies = [
        format!(r#"{{"answers":{{"{q}":{{"type":"choice","choice":"aligned","probabilities":{{"aligned":0.9}}}}}}}}"#),
        format!(r#"{{"answers":{{"{q}":{{"type":"choice","choice":"aligned","probabilities":{{"aligned":0.9,"misaligned":0.1}}}}}}}}"#),
        format!(r#"{{"answers":{{"{q}":{{"type":"choice","choice":"aligned","probabilities":{{"aligned":0.9,"misaligned":0.05,"insufficient_evidence":0.05,"aligned":0.1}}}}}}}}"#),
        format!(r#"{{"answers":{{"{q}":{{"type":"choice","choice":"aligned","probabilities":{{"aligned":0.9,"misaligned":-0.1,"insufficient_evidence":0.05}}}}}}}}"#),
    ];
    for body in bodies {
        let (base, _rx) = serve(Reply::Respond(200, body.clone().into_bytes()));
        assert!(
            matches!(
                judge(&base, &fake_key()).assess(&input("ok")),
                Err(JudgeFailure::Malformed)
            ),
            "{body}"
        );
    }
}

#[test]
fn unknown_question_fields_and_top_level_error_are_malformed() {
    let q = QUESTION_ID;
    let p = probs(0.9, 0.05, 0.05);
    let variants = [
        json!({"answers": {q: {"type": "choice", "choice": "aligned", "probabilities": p, "note": "x"}}}),
        json!({"answers": {q: {"type": "choice", "choice": "aligned", "probabilities": p}, "other": {}}}),
        json!({"error": {"message": "x"}, "answers": {q: {"type": "choice", "choice": "aligned", "probabilities": p}}}),
        json!({"error": null, "answers": {q: {"type": "choice", "choice": "aligned", "probabilities": p}}}),
    ];
    for variant in variants {
        let (base, _rx) = serve(Reply::Respond(200, serde_json::to_vec(&variant).unwrap()));
        assert!(
            matches!(
                judge(&base, &fake_key()).assess(&input("ok")),
                Err(JudgeFailure::Malformed)
            ),
            "{variant}"
        );
    }
}

/// The one real Jev response observed on 2026-11-11 (synthetic input), verbatim.
const LIVE_RESPONSE: &str = r#"{"model":"jev-1.14.0","answers":{"goal_alignment":{"type":"choice","choice":"aligned","confidence":0.62,"probabilities":{"aligned":0.75,"misaligned":0.04,"insufficient_evidence":0.21}}},"usage":{"input_tokens":573,"output_tokens":47}}"#;

#[test]
fn observed_live_response_is_judged() {
    let (base, _rx) = serve(Reply::Respond(200, LIVE_RESPONSE.as_bytes().to_vec()));
    let got = judge(&base, &fake_key()).assess(&input("ok")).unwrap();
    assert_eq!(got.signal, Signal::Aligned);
    for part in ["aligned", "p=0.750", "model=jev-1.14.0", "confidence=0.620", "input_tokens=573", "output_tokens=47"] {
        assert!(got.reason.contains(part), "{} lacks {part}", got.reason);
    }
}

#[test]
fn live_shape_deviations_are_malformed() {
    let live: Value = serde_json::from_str(LIVE_RESPONSE).unwrap();
    let edits: [fn(&mut Value); 9] = [
        |v| v["model"] = Value::Null,
        |v| v["usage"] = Value::Null,
        |v| v["answers"][QUESTION_ID]["confidence"] = Value::Null,
        |v| v["status"] = json!("ok"),
        |v| v["id"] = json!("r-1"),
        |v| v["usage"]["cached_tokens"] = json!(1),
        |v| v["answers"][QUESTION_ID]["confidence"] = json!(1.5),
        |v| v["answers"][QUESTION_ID]["confidence"] = json!(-0.1),
        |v| v["answers"][QUESTION_ID]["confidence"] = json!("0.6"),
    ];
    for edit in edits {
        let mut variant = live.clone();
        edit(&mut variant);
        let (base, _rx) = serve(Reply::Respond(200, serde_json::to_vec(&variant).unwrap()));
        assert!(
            matches!(
                judge(&base, &fake_key()).assess(&input("ok")),
                Err(JudgeFailure::Malformed)
            ),
            "{variant}"
        );
    }
}

#[test]
fn stalled_server_times_out_within_deadline() {
    let (base, _rx) = serve(Reply::Stall(Duration::from_secs(10)));
    let deadline = Duration::from_millis(300);
    let mut judge = JevJudge::with_parts(
        JevConfig {
            deadline,
            ..config(&base)
        },
        Box::new(UreqTransport),
        env_with(Some(fake_key())),
    )
    .unwrap();
    let started = Instant::now();
    let result = judge.assess(&input("ok"));
    let elapsed = started.elapsed();
    assert!(matches!(result, Err(JudgeFailure::Timeout)), "{result:?}");
    assert!(elapsed < deadline + Duration::from_secs(2), "{elapsed:?}");
}

#[test]
fn missing_key_is_unavailable_and_sends_nothing() {
    for key in [None, Some(String::new())] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut judge =
            JevJudge::with_parts(config(&base), Box::new(UreqTransport), env_with(key)).unwrap();
        assert!(matches!(
            judge.assess(&input("ok")),
            Err(JudgeFailure::Unavailable)
        ));
        assert!(judge.last_request_body().is_none());
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn construction_requires_enablement_and_safe_base_url() {
    let key = || env_with(Some(fake_key()));
    let build = |config: JevConfig| JevJudge::with_parts(config, Box::new(UreqTransport), key());
    assert!(build(JevConfig::default()).is_err(), "disabled by default");
    assert!(
        build(JevConfig {
            enabled: false,
            ..config("http://127.0.0.1:1")
        })
        .is_err()
    );
    for refused in [
        "http://example.com",
        "http://127.0.0.1.example.com",
        "http://localhost@example.com",
        "http://10.0.0.1:8080",
        "ftp://127.0.0.1",
        "https://",
        "api.typesafe.ai",
        "https://api.typesafe.ai?x=1",
    ] {
        assert!(build(config(refused)).is_err(), "{refused}");
    }
    for accepted in [
        "https://api.typesafe.ai",
        "https://example.com/",
        "http://127.0.0.1:8080",
        "http://localhost:9",
        "http://[::1]:9",
    ] {
        assert!(build(config(accepted)).is_ok(), "{accepted}");
    }
    let default = JevConfig::default();
    assert_eq!(default.base_url, "https://api.typesafe.ai");
    assert_eq!(default.model, "jev-latest");
    assert!(!default.include_context_bytes);
}

#[test]
fn context_bytes_are_only_sent_when_opted_in() {
    let (base, rx) = serve(Reply::Respond(
        200,
        answer("aligned", probs(0.9, 0.05, 0.05)),
    ));
    judge(&base, &fake_key()).assess(&input("ok")).unwrap();
    let request = rx.recv().unwrap();
    assert!(!contains(&request.body, "SECRET-CONTEXT-MARKER"));
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["state"]["context"][0]["id"], "source-a");
    assert_eq!(
        body["state"]["context"][0]["byte_length"],
        CONTEXT_BYTES.len()
    );

    let (base, rx) = serve(Reply::Respond(
        200,
        answer("aligned", probs(0.9, 0.05, 0.05)),
    ));
    let mut judge = JevJudge::with_parts(
        JevConfig {
            include_context_bytes: true,
            ..config(&base)
        },
        Box::new(UreqTransport),
        env_with(Some(fake_key())),
    )
    .unwrap();
    judge.assess(&input("ok")).unwrap();
    let body: Value = serde_json::from_slice(&rx.recv().unwrap().body).unwrap();
    let source = &body["state"]["context"][0];
    assert_eq!(source["encoding"], "utf8");
    assert_eq!(source["data"], std::str::from_utf8(CONTEXT_BYTES).unwrap());
}

#[test]
fn output_is_truncated_at_cap_on_char_boundary() {
    let (base, rx) = serve(Reply::Respond(
        200,
        answer("aligned", probs(0.9, 0.05, 0.05)),
    ));
    let mut judge = JevJudge::with_parts(
        JevConfig {
            output_cap_bytes: 5,
            ..config(&base)
        },
        Box::new(UreqTransport),
        env_with(Some(fake_key())),
    )
    .unwrap();
    // Byte 5 falls inside the second 2-byte char, so the cut must back off to 4 bytes.
    let output = "abééTAIL-MARKER";
    judge.assess(&input(output)).unwrap();
    let request = rx.recv().unwrap();
    assert!(!contains(&request.body, "TAIL-MARKER"));
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let sent = &body["state"]["observation"]["output"];
    assert_eq!(sent["text"], "abé");
    assert_eq!(sent["truncated"], true);
    assert_eq!(sent["original_byte_length"], output.len());
}

struct Recorder;
impl BranchControl for Recorder {
    fn apply(&self, _: &str, _: &Action) -> io::Result<()> {
        Ok(())
    }
}

fn all_bytes(root: &Path, out: &mut Vec<u8>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all_bytes(&path, out);
        } else {
            out.extend(fs::read(&path).unwrap());
            out.push(b'\n');
        }
    }
}

#[test]
fn key_never_reaches_debug_reason_body_or_monitor_journal() {
    let key = fake_key();
    let (base, rx) = serve(Reply::Respond(
        200,
        answer("misaligned", probs(0.05, 0.9, 0.05)),
    ));
    let mut judge = judge(&base, &key);
    assert!(!format!("{judge:?}").contains(&key));

    let root = TestDir::new();
    let spec = BranchSpec {
        run_id: "jev-run".into(),
        branch_id: "jev-branch".into(),
        owner_session_id: "root".into(),
        session_id: "worker".into(),
        goal: goal(),
        context_ids: vec!["source-a".into()],
        max_corrections: 1,
    };
    let member: SessionInfo = serde_json::from_value(
        json!({"session_id":"worker", "parent_session_id":"root", "status":"running"}),
    )
    .unwrap();
    let mut sources = Memory::open(root.path.join("sources")).unwrap();
    sources.insert("source-a", CONTEXT_BYTES).unwrap();
    let mut monitor = Monitor::open(spec, &member, sources, root.path.join("journal")).unwrap();
    let record = monitor
        .on_event(
            &ApiEvent::ToolDone {
                session_id: "worker".into(),
                call_id: "c1".into(),
                name: "bash".into(),
                output: "edited src/lib.rs".into(),
                error: None,
            },
            &mut judge,
            &Recorder,
        )
        .unwrap()
        .unwrap();
    assert_eq!(record.assessment.signal, Signal::Misaligned);
    assert!(record.assessment.reason.contains("jev-2026-10-01"));
    assert!(!record.assessment.reason.contains(&key));
    assert!(!format!("{judge:?}").contains(&key));

    let request = rx.recv().unwrap();
    assert!(request.head.contains(&key), "server saw the bearer key");
    assert!(!contains(&request.body, &key));
    assert!(!contains(judge.last_request_body().unwrap(), &key));

    let mut journal = Vec::new();
    all_bytes(&root.path, &mut journal);
    assert!(!journal.is_empty());
    assert!(!contains(&journal, &key));
}
