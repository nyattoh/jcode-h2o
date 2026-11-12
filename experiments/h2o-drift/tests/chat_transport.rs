#![cfg(feature = "sdk")]
//! ChatController over a REAL JcodeClient and a scripted in-memory harness peer.

mod chat_support;
mod support;

use chat_support::*;
use h2o_drift_prototype::chat_transport::{CHAT_SYSTEM_PROMPT, ChatController, ChatEvent, ConnectConfig};
use jcode_sdk::{ApiEvent, ApiRequest, PermissionDecision, TurnStopReason, api::ErrorCode};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(5);

fn sends(h: &Harness) -> usize {
    h.count(|r| matches!(r, ApiRequest::SendMessage { .. }))
}

fn failed(events: &[ChatEvent]) -> (Option<u64>, Option<u64>, String, bool) {
    events
        .iter()
        .find_map(|e| match e {
            ChatEvent::Failed { conversation_id, request_id, message, uncertain } => {
                Some((*conversation_id, *request_id, message.clone(), *uncertain))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no Failed event in {events:#?}"))
}

fn replies(events: &[ChatEvent]) -> Vec<(u64, u64, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Reply { conversation_id, request_id, text } => Some((*conversation_id, *request_id, text.clone())),
            _ => None,
        })
        .collect()
}

fn script_turn(f: impl Fn(&str, &str) -> Vec<Step> + Send + Sync + 'static) -> Script {
    Script { turn: Arc::new(f), ..Script::default() }
}

// --- setup / tool enforcement ---

#[test]
fn setup_order_is_hello_create_configure_verify_before_any_prompt() {
    let h = Harness::new(Script::default());
    let mut c = h.controller();
    let events = connect_ready(&h, &mut c, 7);
    assert!(matches!(events.first(), Some(ChatEvent::Connecting)), "{events:#?}");
    match events.last().unwrap() {
        ChatEvent::Ready { conversation_id, session_id, provider, model } => {
            assert_eq!(*conversation_id, 7);
            assert_eq!(session_id, "sess-0");
            assert_eq!(provider.as_deref(), Some("scripted-provider"));
            assert_eq!(model.as_deref(), Some("scripted-model"));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
    // Nothing is sent without an explicit user Send.
    assert!(drain(&mut c, Duration::from_millis(200)).is_empty());
    assert_eq!(sends(&h), 0);

    let reqs = h.requests();
    let pos = |pred: &dyn Fn(&ApiRequest) -> bool| reqs.iter().position(|r| pred(r)).unwrap();
    let hello = pos(&|r| matches!(r, ApiRequest::Hello { supports_user_response: false, .. }));
    let create = pos(&|r| matches!(r, ApiRequest::CreateSession { .. }));
    let configure = pos(&|r| matches!(r, ApiRequest::ConfigureTools { .. }));
    let list = pos(&|r| matches!(r, ApiRequest::ListTools { .. }));
    assert!(hello < create && create < configure && configure < list, "{reqs:#?}");
    assert_eq!(h.endpoints(), vec![std::path::PathBuf::from(TEST_ENDPOINT)]);

    for r in &reqs {
        match r {
            ApiRequest::CreateSession { working_dir, system_prompt } => {
                assert_eq!(system_prompt.as_deref(), Some(CHAT_SYSTEM_PROMPT));
                let dir = std::path::PathBuf::from(working_dir.as_deref().expect("explicit neutral working_dir"));
                assert!(dir.is_absolute(), "{dir:?}");
                assert!(!dir.starts_with(std::env::current_dir().unwrap()), "must not use the process cwd: {dir:?}");
                assert!(!dir.starts_with(env!("CARGO_MANIFEST_DIR")), "must not use the project: {dir:?}");
                let entries = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
                assert_eq!(entries, 0, "neutral dir must be empty");
            }
            ApiRequest::ConfigureTools { session_id, tools } => {
                assert_eq!(session_id, "sess-0");
                assert_eq!(tools.enabled, Some(vec![]));
                assert!(tools.disabled.is_empty() && tools.custom.is_empty());
            }
            ApiRequest::Hello { .. }
            | ApiRequest::ListTools { .. }
            | ApiRequest::Ping
            | ApiRequest::GetRuntimeInfo { .. } => {}
            other => panic!("unexpected request during setup: {}", serde_json::to_string(other).unwrap()),
        }
    }
}

fn assert_setup_refused(script: Script, needle: &str, max_requests: &[&str]) {
    let h = Harness::new(script);
    let mut c = h.controller();
    c.connect(h.config(), 1).unwrap();
    let events = wait_for(&mut c, WAIT, is_disconnected);
    let (conv, req, message, uncertain) = failed(&events);
    assert_eq!((conv, req, uncertain), (Some(1), None, false), "{events:#?}");
    assert!(message.contains(needle), "{message}");
    assert!(!events.iter().any(is_ready), "{events:#?}");
    assert!(c.send(1, 1, "hello".into()).is_err());
    assert_eq!(sends(&h), 0);
    for r in h.requests() {
        let name = serde_json::to_value(&r).unwrap()["req"].as_str().unwrap().to_string();
        assert!(max_requests.contains(&name.as_str()), "unexpected {name} in refused setup");
    }
}

#[test]
fn missing_session_tools_capability_fails_closed_before_creating_a_session() {
    let mut script = Script::default();
    script.capabilities.retain(|c| c != "session_tools");
    assert_setup_refused(script, "session_tools", &["hello"]);
}

#[test]
fn configure_tools_error_fails_closed() {
    let script = Script { configure: Reply::Error, ..Script::default() };
    assert_setup_refused(script, "configure_tools failure", &["hello", "create_session", "configure_tools"]);
}

#[test]
fn effective_tools_not_empty_fails_closed() {
    let script = Script { effective_tools: vec!["bash".into()], ..Script::default() };
    assert_setup_refused(script, "bash", &["hello", "create_session", "configure_tools", "list_tools"]);
}

#[test]
fn silent_configure_ack_is_bounded_and_fails_closed() {
    let script = Script { configure: Reply::Silent, ..Script::default() };
    let h = Harness::new(script);
    let mut c = h.controller();
    let mut cfg = h.config();
    cfg.request_timeout = Duration::from_millis(150);
    let start = Instant::now();
    c.connect(cfg, 1).unwrap();
    let events = wait_for(&mut c, WAIT, is_disconnected);
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    let (_, _, _, uncertain) = failed(&events);
    assert!(!uncertain);
    assert_eq!(sends(&h), 0);
}

#[test]
fn route_is_only_what_the_harness_reported() {
    let mut script = Script::default();
    script.capabilities.retain(|c| c != "runtime_info");
    let h = Harness::new(script);
    let mut c = h.controller();
    let events = connect_ready(&h, &mut c, 1);
    assert!(
        matches!(events.last(), Some(ChatEvent::Ready { provider: None, model: None, .. })),
        "{events:#?}"
    );
}

#[test]
fn stuck_transport_is_bounded_by_the_controller_watchdog_without_blocking_caller() {
    let mut c = ChatController::with_transport_factory(|_| {
        thread::sleep(Duration::from_secs(3));
        Err(jcode_sdk::Error::new(jcode_sdk::ErrorKind::ConnectFailed, "late"))
    });
    let cfg = ConnectConfig {
        endpoint: "stuck".into(),
        request_timeout: Duration::from_millis(100),
        turn_timeout: Duration::from_secs(1),
    };
    let start = Instant::now();
    c.connect(cfg, 3).unwrap();
    assert!(start.elapsed() < Duration::from_millis(50), "connect blocked {:?}", start.elapsed());
    let events = wait_for(&mut c, Duration::from_secs(2), is_disconnected);
    assert!(start.elapsed() < Duration::from_secs(2));
    let (conv, req, _, uncertain) = failed(&events);
    assert_eq!((conv, req, uncertain), (Some(3), None, false));
    // The late worker result must never surface.
    assert!(drain(&mut c, Duration::from_millis(3200)).is_empty());
}

#[test]
fn default_controller_uses_explicit_endpoint_only_and_never_starts_a_runtime() {
    let dir = support::TestDir::new();
    let endpoint = dir.path.join("no-harness-here.sock");
    let mut c = ChatController::new();
    assert!(c.connect(ConnectConfig::default(), 1).is_err(), "empty default endpoint must be rejected");
    c.connect(
        ConnectConfig { endpoint: endpoint.clone(), request_timeout: Duration::from_secs(1), turn_timeout: Duration::from_secs(1) },
        1,
    )
    .unwrap();
    let events = wait_for(&mut c, WAIT, is_disconnected);
    let (_, _, message, uncertain) = failed(&events);
    assert!(!uncertain);
    assert!(message.contains("no-harness-here"), "actionable message naming the endpoint: {message}");
    assert!(!endpoint.exists(), "no runtime may be started at the endpoint");
    assert_eq!(std::fs::read_dir(&dir.path).unwrap().count(), 0);
}

#[test]
fn local_invalid_and_busy_conditions_are_rejected() {
    let h = Harness::new(script_turn(|s, _| {
        let mut steps = vec![Step::Sleep(Duration::from_millis(400))];
        steps.extend(reply_steps(s, "late"));
        steps
    }));
    let mut c = h.controller();
    assert!(c.send(1, 1, "x".into()).is_err(), "send before connect");
    assert!(c.new_session(1).is_err(), "new_session before connect");
    let mut zero = h.config();
    zero.request_timeout = Duration::ZERO;
    assert!(c.connect(zero, 1).is_err());
    connect_ready(&h, &mut c, 1);
    assert!(c.send(1, 1, "  \n".into()).is_err(), "blank text");
    assert!(c.send(2, 1, "x".into()).is_err(), "conversation without session");
    c.send(1, 1, "x".into()).unwrap();
    assert!(c.send(1, 2, "y".into()).is_err(), "busy");
    assert!(c.new_session(2).is_err(), "busy");
    assert!(c.connect(h.config(), 2).is_err(), "busy");
    let events = wait_for(&mut c, WAIT, is_terminal);
    assert_eq!(replies(&events), vec![(1, 1, "late".to_string())]);
    assert_eq!(sends(&h), 1);
}

// --- replies and framing ---

#[test]
fn send_produces_exactly_one_real_sdk_reply() {
    let h = Harness::new(script_turn(|s, content| {
        let mut steps = reply_steps(s, &format!("echo: {content}"));
        steps.push(turn_done(s)); // a duplicate terminal frame must not duplicate the reply
        steps
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 4);
    let start = Instant::now();
    c.send(4, 11, "こんにちは".into()).unwrap();
    assert!(start.elapsed() < Duration::from_millis(50));
    let mut events = wait_for(&mut c, WAIT, is_terminal);
    events.extend(drain(&mut c, Duration::from_millis(300)));
    assert_eq!(replies(&events), vec![(4, 11, "echo: こんにちは".to_string())], "{events:#?}");
    assert!(!events.iter().any(|e| matches!(e, ChatEvent::Failed { .. })));
    let sent: Vec<_> = h
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            ApiRequest::SendMessage { session_id, content, system_reminder, images, .. } => {
                Some((session_id, content, system_reminder, images.len()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![("sess-0".into(), "こんにちは".into(), None, 0)]);
    // Session stays usable.
    c.send(4, 12, "again".into()).unwrap();
    let events = wait_for(&mut c, WAIT, is_terminal);
    assert_eq!(replies(&events), vec![(4, 12, "echo: again".to_string())]);
}

#[test]
fn framing_replacement_and_retraction_select_final_text() {
    let h = Harness::new(script_turn(|s, _| {
        vec![
            delta(s, Some("m1"), "draft"),
            done(s, Some("m1")),
            replace(s, Some("m1"), ""), // retracted after a provider retry
            ev(ApiEvent::ReasoningDelta { session_id: s.into(), text: "thinking".into() }),
            delta(s, Some("m2"), "fin"),
            delta(s, Some("m2"), "al-typo"),
            replace(s, Some("m2"), "final"),
            done(s, Some("m2")),
            turn_done(s),
        ]
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    let events = wait_for(&mut c, WAIT, is_terminal);
    assert_eq!(replies(&events), vec![(1, 1, "final".to_string())]);
}

#[test]
fn unframed_bridge_falls_back_to_aggregate_text() {
    let h = Harness::new(script_turn(|s, _| vec![delta(s, None, "a"), delta(s, None, "b"), turn_done(s)]));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    assert_eq!(replies(&wait_for(&mut c, WAIT, is_terminal)), vec![(1, 1, "ab".to_string())]);
}

#[test]
fn fully_retracted_or_empty_turn_is_failed_not_an_empty_reply() {
    let h = Harness::new(script_turn(|s, _| {
        vec![delta(s, Some("m1"), "oops"), done(s, Some("m1")), replace(s, Some("m1"), ""), turn_done(s)]
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 5, "q".into()).unwrap();
    let events = wait_for(&mut c, WAIT, is_terminal);
    let (conv, req, _, uncertain) = failed(&events);
    assert_eq!((conv, req, uncertain), (Some(1), Some(5), false));
    assert!(replies(&events).is_empty());
    // Definite failure keeps the session.
    c.send(1, 6, "again".into()).unwrap();
    assert!(matches!(wait_for(&mut c, WAIT, is_terminal).last(), Some(ChatEvent::Failed { request_id: Some(6), .. })));
}

#[test]
fn stopped_or_errored_turn_is_failed_without_substitute_reply() {
    let h = Harness::new(script_turn(|s, _| {
        vec![
            delta(s, Some("m1"), "partial"),
            ev(ApiEvent::TurnStopped {
                session_id: s.into(),
                reason: TurnStopReason::Failure,
                message: "provider overloaded".into(),
                provider_stop_reason: None,
            }),
            ev(ApiEvent::Error { code: ErrorCode::Internal, message: "provider overloaded".into() }),
        ]
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    let mut events = wait_for(&mut c, WAIT, is_terminal);
    events.extend(drain(&mut c, Duration::from_millis(200)));
    let (_, req, message, uncertain) = failed(&events);
    assert_eq!((req, uncertain), (Some(1), false));
    assert!(message.contains("provider overloaded"), "{message}");
    assert!(replies(&events).is_empty());
    assert!(!events.iter().any(is_disconnected));
}

#[test]
fn unexplained_error_mid_turn_is_uncertain_and_retires_session() {
    let h = Harness::new(script_turn(|_, _| {
        vec![ev(ApiEvent::Error { code: ErrorCode::Internal, message: "bridge exploded".into() })]
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    let events = wait_for(&mut c, WAIT, is_disconnected);
    assert_eq!(failed(&events).3, true);
    assert!(c.send(1, 2, "q".into()).is_err());
}

// --- tool / permission violations ---

#[test]
fn tool_attempt_despite_exclusion_fails_closed_and_cancels() {
    let h = Harness::new(script_turn(|s, _| {
        vec![
            ev(ApiEvent::ToolStart { session_id: s.into(), call_id: "c1".into(), name: "bash".into() }),
            Step::Sleep(Duration::from_millis(200)),
            delta(s, Some("m1"), "I ran it"),
            done(s, Some("m1")),
            turn_done(s),
        ]
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    let mut events = wait_for(&mut c, WAIT, is_disconnected);
    events.extend(drain(&mut c, Duration::from_millis(400)));
    let (_, req, message, uncertain) = failed(&events);
    assert_eq!((req, uncertain), (Some(1), true));
    assert!(message.contains("bash"), "{message}");
    assert!(replies(&events).is_empty(), "{events:#?}");
    assert_eq!(h.count(|r| matches!(r, ApiRequest::Cancel { .. })), 1);
}

#[test]
fn permission_request_is_denied_never_auto_approved() {
    let h = Harness::new(script_turn(|s, _| {
        vec![ev(ApiEvent::PermissionRequest {
            session_id: s.into(),
            request_id: "p1".into(),
            tool_name: "write".into(),
            description: "write a file".into(),
        })]
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    let events = wait_for(&mut c, WAIT, is_disconnected);
    assert!(failed(&events).3);
    let decisions: Vec<_> = h
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            ApiRequest::PermissionResponse { decision, .. } => Some(decision),
            _ => None,
        })
        .collect();
    assert!(!decisions.is_empty() && decisions.iter().all(|d| *d == PermissionDecision::Deny), "{decisions:?}");
}

// --- bounds, cancel, disconnect, uncertainty ---

#[test]
fn turn_timeout_is_uncertain_and_never_blindly_resent() {
    let h = Harness::new(script_turn(|s, _| {
        let mut steps = vec![Step::Sleep(Duration::from_millis(1500))];
        steps.extend(reply_steps(s, "too late"));
        steps
    }));
    let mut c = h.controller();
    let mut cfg = h.config();
    cfg.request_timeout = Duration::from_millis(200);
    cfg.turn_timeout = Duration::from_millis(200);
    c.connect(cfg, 1).unwrap();
    wait_for(&mut c, WAIT, is_ready);
    let start = Instant::now();
    c.send(1, 9, "q".into()).unwrap();
    let mut events = wait_for(&mut c, WAIT, is_disconnected);
    assert!(start.elapsed() < Duration::from_millis(1400), "{:?}", start.elapsed());
    events.extend(drain(&mut c, Duration::from_millis(1500)));
    let (conv, req, _, uncertain) = failed(&events);
    assert_eq!((conv, req, uncertain), (Some(1), Some(9), true));
    assert!(replies(&events).is_empty(), "{events:#?}");
    assert!(c.send(1, 10, "q".into()).is_err(), "no send after uncertain outcome until reconnect");
    assert_eq!(sends(&h), 1);
}

#[test]
fn confirmed_cancel_ends_turn_and_keeps_session() {
    let h = Harness::new(script_turn(|s, _| {
        let mut steps = vec![delta(s, Some("m1"), "part"), Step::Sleep(Duration::from_millis(1500))];
        steps.extend(reply_steps(s, "not shown"));
        steps
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    c.cancel();
    assert!(start.elapsed() < Duration::from_millis(50));
    let mut events = wait_for(&mut c, WAIT, is_terminal);
    events.extend(drain(&mut c, Duration::from_millis(200)));
    let (_, req, _, uncertain) = failed(&events);
    assert_eq!((req, uncertain), (Some(1), false));
    assert!(replies(&events).is_empty());
    assert!(!events.iter().any(is_disconnected), "{events:#?}");
    assert_eq!(h.count(|r| matches!(r, ApiRequest::Cancel { .. })), 1);
    c.send(1, 2, "next".into()).unwrap();
    assert!(matches!(wait_for(&mut c, WAIT, is_terminal).last(), Some(ChatEvent::Reply { request_id: 2, .. })));
}

#[test]
fn unconfirmed_cancel_is_uncertain_and_bounded() {
    let h = Harness::new(Script {
        cancel_ends_turn: false,
        turn: Arc::new(|_, _| vec![Step::Sleep(Duration::from_secs(3))]),
        ..Script::default()
    });
    let mut c = h.controller();
    let mut cfg = h.config();
    cfg.request_timeout = Duration::from_millis(200);
    c.connect(cfg, 1).unwrap();
    wait_for(&mut c, WAIT, is_ready);
    c.send(1, 1, "q".into()).unwrap();
    // fix1 adaptation: the prompt must have been delivered, otherwise the cancel is (correctly)
    // confirmed locally as "nothing was sent" — see each_cancel_binds_to_its_own_request_...
    let sent_by = Instant::now() + WAIT;
    while sends(&h) == 0 && Instant::now() < sent_by {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(sends(&h), 1);
    let start = Instant::now();
    c.cancel();
    let events = wait_for(&mut c, WAIT, is_disconnected);
    assert!(start.elapsed() < Duration::from_millis(1500), "{:?}", start.elapsed());
    assert_eq!(failed(&events).3, true);
}

#[test]
fn peer_disconnect_after_receiving_prompt_is_uncertain() {
    let h = Harness::new(script_turn(|_, _| vec![Step::Disconnect]));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    let events = wait_for(&mut c, WAIT, is_disconnected);
    let (_, req, _, uncertain) = failed(&events);
    assert_eq!((req, uncertain), (Some(1), true));
    assert!(c.send(1, 2, "q".into()).is_err());
    assert!(c.new_session(1).is_err());
    assert_eq!(sends(&h), 1);
}

#[test]
fn idle_peer_disconnect_is_detected() {
    let h = Harness::new(Script::default());
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    h.drop_connection(0);
    let events = wait_for(&mut c, WAIT, is_disconnected);
    assert!(!events.iter().any(|e| matches!(e, ChatEvent::Failed { .. })), "{events:#?}");
    assert!(c.send(1, 1, "q".into()).is_err());
}

#[test]
fn user_disconnect_mid_turn_is_immediate_uncertain_and_drops_late_reply() {
    let h = Harness::new(script_turn(|s, _| {
        let mut steps = vec![Step::Sleep(Duration::from_millis(300))];
        steps.extend(reply_steps(s, "late"));
        steps
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    let start = Instant::now();
    c.disconnect();
    assert!(start.elapsed() < Duration::from_millis(50));
    let first = c.try_recv();
    let second = c.try_recv();
    assert!(matches!(first, Some(ChatEvent::Failed { request_id: Some(1), uncertain: true, .. })), "{first:?}");
    assert!(matches!(second, Some(ChatEvent::Disconnected { .. })), "{second:?}");
    assert!(drain(&mut c, Duration::from_millis(600)).is_empty());
    c.disconnect();
    assert!(c.try_recv().is_none(), "disconnect when disconnected is a no-op");
}

// --- isolation / generations ---

#[test]
fn each_conversation_gets_its_own_isolated_session_and_connection() {
    let h = Harness::new(script_turn(|s, content| reply_steps(s, &format!("{s}:{content}"))));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "a".into()).unwrap();
    assert_eq!(replies(&wait_for(&mut c, WAIT, is_terminal)), vec![(1, 1, "sess-0:a".to_string())]);

    c.new_session(2).unwrap();
    let events = wait_for(&mut c, WAIT, is_ready);
    assert!(
        matches!(events.last(), Some(ChatEvent::Ready { conversation_id: 2, session_id, .. }) if session_id == "sess-1"),
        "{events:#?}"
    );
    assert!(c.send(1, 2, "old".into()).is_err(), "old conversation is not bound to the new session");
    c.send(2, 3, "b".into()).unwrap();
    assert_eq!(replies(&wait_for(&mut c, WAIT, is_terminal)), vec![(2, 3, "sess-1:b".to_string())]);

    assert_eq!(h.endpoints().len(), 2);
    for conn in 0..2 {
        let reqs = h.requests_on(conn);
        assert!(!reqs.iter().any(|r| matches!(r, ApiRequest::AttachSession { .. })));
        let configure = reqs.iter().position(|r| matches!(r, ApiRequest::ConfigureTools { .. })).unwrap();
        if let Some(send) = reqs.iter().position(|r| matches!(r, ApiRequest::SendMessage { .. })) {
            assert!(configure < send);
        }
    }
}

#[test]
fn stale_generation_events_are_ignored() {
    let h = Harness::new(Script::default());
    let mut c = h.controller();
    c.connect(h.config(), 1).unwrap();
    c.connect(h.config(), 2).unwrap();
    let mut events = wait_for(&mut c, WAIT, is_ready);
    events.extend(drain(&mut c, Duration::from_millis(300)));
    let ready: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Ready { conversation_id, .. } => Some(*conversation_id),
            _ => None,
        })
        .collect();
    assert_eq!(ready, vec![2], "{events:#?}");
}

#[test]
fn only_chat_protocol_requests_ever_leave_the_controller() {
    let h = Harness::new(Script::default());
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    wait_for(&mut c, WAIT, is_terminal);
    c.disconnect();
    let allowed = [
        "hello",
        "create_session",
        "configure_tools",
        "list_tools",
        "ping",
        "get_runtime_info",
        "send_message",
        "cancel",
        "permission_response",
    ];
    for r in h.requests() {
        let name = serde_json::to_value(&r).unwrap()["req"].as_str().unwrap().to_string();
        assert!(allowed.contains(&name.as_str()), "unexpected outbound request {name}");
    }
}

// --- fix round 1: turn isolation (R1/R3), cancel identity (R2), cancel-time violations ---
// The first three are the independent reviewer probes (review-r1-probes.rs), same logic.

fn probe_terminal(e: &ChatEvent) -> bool {
    matches!(e, ChatEvent::Reply { .. } | ChatEvent::Failed { request_id: Some(_), .. })
}

#[test]
fn delayed_old_frames_must_not_become_next_reply() {
    let h = Harness::new(script_turn(|s, q| {
        if q == "first" {
            let mut v = reply_steps(s, "first answer");
            v.push(Step::Sleep(Duration::from_millis(350)));
            v.extend(reply_steps(s, "OLD late answer"));
            v
        } else {
            let mut v = vec![Step::Sleep(Duration::from_millis(750))];
            v.extend(reply_steps(s, "second answer"));
            v
        }
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "first".into()).unwrap();
    wait_for(&mut c, Duration::from_secs(3), probe_terminal);
    c.send(1, 2, "second".into()).unwrap();
    let e = wait_for(&mut c, Duration::from_secs(3), probe_terminal);
    assert!(
        e.iter().any(|e| matches!(e, ChatEvent::Reply { request_id: 2, text, .. } if text == "second answer")),
        "old turn became next response: {e:?}"
    );
}

#[test]
fn actual_bridge_error_tail_must_not_end_next_turn() {
    let h = Harness::new(script_turn(|s, q| {
        if q == "first" {
            vec![
                delta(s, Some("old"), "old partial"),
                ev(ApiEvent::TurnStopped {
                    session_id: s.into(),
                    reason: TurnStopReason::Failure,
                    message: "failed".into(),
                    provider_stop_reason: None,
                }),
                ev(ApiEvent::Error { code: ErrorCode::Internal, message: "failed".into() }),
                Step::Sleep(Duration::from_millis(350)),
                done(s, Some("old")),
                turn_done(s),
            ]
        } else {
            let mut v = vec![Step::Sleep(Duration::from_millis(750))];
            v.extend(reply_steps(s, "second answer"));
            v
        }
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "first".into()).unwrap();
    let first = wait_for(&mut c, Duration::from_secs(3), probe_terminal);
    assert!(!failed(&first).3, "a reported turn failure is definite: {first:?}");
    c.send(1, 2, "second".into()).unwrap();
    let e = wait_for(&mut c, Duration::from_secs(3), probe_terminal);
    assert!(
        e.iter().any(|e| matches!(e, ChatEvent::Reply { request_id: 2, text, .. } if text == "second answer")),
        "failed turn tail misattributed: {e:?}"
    );
}

#[test]
fn immediate_cancel_must_not_be_lost() {
    let h = Harness::new(script_turn(|s, _| {
        let mut v = vec![Step::Sleep(Duration::from_millis(800))];
        v.extend(reply_steps(s, "should cancel"));
        v
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    c.cancel();
    let mut e = wait_for(&mut c, Duration::from_secs(3), probe_terminal);
    assert!(
        e.iter().any(|e| matches!(e, ChatEvent::Failed { message, .. } if message.contains("cancel"))),
        "cancel lost: {e:?}"
    );
    e.extend(drain(&mut c, Duration::from_millis(1000)));
    assert!(replies(&e).is_empty(), "{e:?}");
}

#[test]
fn each_cancel_binds_to_its_own_request_and_is_never_cleared() {
    let h = Harness::new(script_turn(|s, q| {
        let mut v = vec![];
        if q.starts_with("slow") {
            v.push(Step::Sleep(Duration::from_millis(800)));
        }
        v.extend(reply_steps(s, &format!("answer to {q}")));
        v
    }));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    for (req, text) in [(1, "slow one"), (2, "slow two")] {
        c.send(1, req, text.into()).unwrap();
        c.cancel();
        let e = wait_for(&mut c, Duration::from_secs(3), probe_terminal);
        let (_, r, message, uncertain) = failed(&e);
        assert_eq!((r, uncertain), (Some(req), false), "{e:?}");
        assert!(message.contains("cancel"), "{message}");
    }
    c.send(1, 3, "plain".into()).unwrap();
    let mut e = wait_for(&mut c, Duration::from_secs(3), probe_terminal);
    e.extend(drain(&mut c, Duration::from_millis(1000)));
    assert_eq!(replies(&e), vec![(1, 3, "answer to plain".to_string())], "an old cancel leaked: {e:?}");
    assert!(!e.iter().any(is_disconnected), "{e:?}");
}

#[test]
fn bare_error_while_cancelling_is_not_a_cancel_confirmation() {
    let h = Harness::new(Script {
        cancel_ack_only: true,
        turn: Arc::new(|s, _| {
            vec![
                delta(s, Some("m1"), "part"),
                Step::Sleep(Duration::from_millis(300)),
                ev(ApiEvent::Error { code: ErrorCode::Internal, message: "unrelated".into() }),
            ]
        }),
        ..Script::default()
    });
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    thread::sleep(Duration::from_millis(100));
    c.cancel();
    let e = wait_for(&mut c, WAIT, probe_terminal);
    let (_, req, _, uncertain) = failed(&e);
    assert_eq!((req, uncertain), (Some(1), true), "an unattributable error must not confirm the cancel: {e:?}");
    wait_for(&mut c, WAIT, is_disconnected);
    assert!(c.send(1, 2, "q".into()).is_err());
}

#[test]
fn reported_stop_then_error_while_cancelling_confirms_the_cancel() {
    // Not a RED case: guards the bridge's real interrupted-turn shape against over-correction.
    let h = Harness::new(Script {
        cancel_ack_only: true,
        turn: Arc::new(|s, q| {
            if q != "q" {
                return reply_steps(s, "after cancel");
            }
            vec![
                delta(s, Some("m1"), "part"),
                Step::Sleep(Duration::from_millis(300)),
                ev(ApiEvent::TurnStopped {
                    session_id: s.into(),
                    reason: TurnStopReason::Interrupted,
                    message: "interrupted".into(),
                    provider_stop_reason: None,
                }),
                ev(ApiEvent::Error { code: ErrorCode::Internal, message: "interrupted".into() }),
            ]
        }),
        ..Script::default()
    });
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    c.send(1, 1, "q".into()).unwrap();
    thread::sleep(Duration::from_millis(100));
    c.cancel();
    let e = wait_for(&mut c, WAIT, probe_terminal);
    assert!(!failed(&e).3, "{e:?}");
    c.send(1, 2, "next".into()).unwrap();
    assert_eq!(replies(&wait_for(&mut c, WAIT, probe_terminal)), vec![(1, 2, "after cancel".to_string())]);
}

#[test]
fn tool_or_permission_attempt_while_cancelling_fails_closed() {
    let tool: TurnScript = Arc::new(|s, _| {
        vec![
            Step::Sleep(Duration::from_millis(300)),
            ev(ApiEvent::ToolStart { session_id: s.into(), call_id: "c1".into(), name: "bash".into() }),
            turn_done(s),
        ]
    });
    let permission: TurnScript = Arc::new(|s, _| {
        vec![
            Step::Sleep(Duration::from_millis(300)),
            ev(ApiEvent::PermissionRequest {
                session_id: s.into(),
                request_id: "p1".into(),
                tool_name: "write".into(),
                description: "write a file".into(),
            }),
            turn_done(s),
        ]
    });
    for (turn, needle) in [(tool, "bash"), (permission, "write")] {
        let h = Harness::new(Script { cancel_ack_only: true, turn, ..Script::default() });
        let mut c = h.controller();
        connect_ready(&h, &mut c, 1);
        c.send(1, 1, "q".into()).unwrap();
        thread::sleep(Duration::from_millis(100));
        c.cancel();
        let e = wait_for(&mut c, WAIT, probe_terminal);
        let (_, req, message, uncertain) = failed(&e);
        assert_eq!((req, uncertain), (Some(1), true), "{needle}: {e:?}");
        assert!(message.contains(needle), "{message}");
        wait_for(&mut c, WAIT, is_disconnected);
        let decisions: Vec<_> = h
            .requests()
            .into_iter()
            .filter_map(|r| match r {
                ApiRequest::PermissionResponse { decision, .. } => Some(decision),
                _ => None,
            })
            .collect();
        assert!(decisions.iter().all(|d| *d == PermissionDecision::Deny), "{decisions:?}");
        if needle == "write" {
            assert_eq!(decisions.len(), 1, "permission must be answered (Deny)");
        }
    }
}

const REJECTED: &str = "ZZ-rejected-prompt";

#[test]
fn consecutive_turns_continue_the_conversation_in_fresh_isolated_sessions() {
    let h = Harness::new(script_turn(|s, q| {
        if q == REJECTED {
            vec![turn_done(s)] // definite failure without text: must never be replayed
        } else {
            reply_steps(s, &format!("{s} says {q}"))
        }
    }));
    let mut c = h.controller();
    let mut events = connect_ready(&h, &mut c, 1);
    for (req, text) in [(1, "alpha"), (2, REJECTED), (3, "beta"), (4, "gamma")] {
        c.send(1, req, text.into()).unwrap();
        events.extend(wait_for(&mut c, WAIT, probe_terminal));
    }
    events.extend(drain(&mut c, Duration::from_millis(200)));
    assert_eq!(
        replies(&events),
        vec![
            (1, 1, "sess-0 says alpha".to_string()),
            (1, 3, "sess-2 says beta".to_string()),
            (1, 4, "sess-3 says gamma".to_string()),
        ],
        "{events:#?}"
    );
    assert_eq!(failed(&events).1, Some(2));
    assert_eq!(events.iter().filter(|e| is_ready(e)).count(), 1, "no extra Ready: {events:#?}");
    assert!(!events.iter().any(is_disconnected), "{events:#?}");

    // One SendMessage per user Send, each the only prompt of its own session/connection, with the
    // user's exact text as content and only accepted earlier turns as the bounded replay.
    let sent: Vec<_> = h
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            ApiRequest::SendMessage { session_id, content, system_reminder, no_reply, .. } => {
                assert!(!no_reply);
                Some((session_id, content, system_reminder))
            }
            _ => None,
        })
        .collect();
    let ids: Vec<_> = sent.iter().map(|(s, c, _)| (s.as_str(), c.as_str())).collect();
    assert_eq!(ids, [("sess-0", "alpha"), ("sess-1", REJECTED), ("sess-2", "beta"), ("sess-3", "gamma")]);
    assert_eq!(sent[0].2, None, "first turn has no replay");
    for (_, _, reminder) in &sent[1..] {
        let r = reminder.as_deref().expect("later turns replay the accepted transcript");
        assert!(r.contains("<<<BEGIN EARLIER TURNS") && r.contains("<<<END EARLIER TURNS>>>"), "{r}");
        assert!(r.contains("alpha") && r.contains("sess-0 says alpha"), "{r}");
        assert!(!r.contains(REJECTED), "failed prompt replayed: {r}");
    }
    let last = sent[3].2.as_deref().unwrap();
    assert!(last.contains("sess-2 says beta") && last.find("alpha") < last.find("beta"), "{last}");
    assert!(!sent[2].2.as_deref().unwrap().contains("beta"), "the new message is not duplicated into the replay");

    assert_eq!(h.endpoints().len(), 4);
    let mut dirs = vec![];
    for conn in 0..4 {
        let reqs = h.requests_on(conn);
        let at = |pred: &dyn Fn(&ApiRequest) -> bool| reqs.iter().position(|r| pred(r));
        let send = at(&|r| matches!(r, ApiRequest::SendMessage { .. })).unwrap();
        let configure = at(&|r| {
            matches!(r, ApiRequest::ConfigureTools { tools, .. }
                if tools.enabled == Some(vec![]) && tools.custom.is_empty())
        })
        .unwrap();
        let list = at(&|r| matches!(r, ApiRequest::ListTools { .. })).unwrap();
        assert!(configure < list && list < send, "conn {conn}: {reqs:#?}");
        assert_eq!(reqs.iter().filter(|r| matches!(r, ApiRequest::SendMessage { .. })).count(), 1);
        assert!(!reqs.iter().any(|r| matches!(r, ApiRequest::AttachSession { .. })));
        for r in &reqs {
            if let ApiRequest::CreateSession { working_dir, system_prompt } = r {
                assert_eq!(system_prompt.as_deref(), Some(CHAT_SYSTEM_PROMPT));
                dirs.push(working_dir.clone().unwrap());
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    assert_eq!(dirs.len(), 4, "each session has its own neutral dir");
}

#[test]
fn replay_is_bounded_to_recent_whole_turns() {
    let big = "x".repeat(20_000);
    let h = Harness::new(script_turn(|s, q| reply_steps(s, &format!("ok {}", &q[..q.len().min(8)]))));
    let mut c = h.controller();
    connect_ready(&h, &mut c, 1);
    for (req, text) in [(1, format!("one{big}")), (2, format!("two{big}")), (3, "three".to_string())] {
        c.send(1, req, text).unwrap();
        wait_for(&mut c, WAIT, probe_terminal);
    }
    let reminders: Vec<String> = h
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            ApiRequest::SendMessage { system_reminder, .. } => system_reminder,
            _ => None,
        })
        .collect();
    assert_eq!(reminders.len(), 2, "{}", reminders.len());
    let last = reminders.last().unwrap();
    assert!(last.chars().count() <= 33_000, "{}", last.chars().count());
    assert!(last.contains("two") && !last.contains("one"), "keeps the most recent whole turn only");
    assert!(last.contains("1 older turn"), "omission is stated: {}", &last[..200]);
}

#[test]
fn later_turn_route_change_or_reused_session_is_refused_before_sending() {
    let route_change = Script {
        model_for: Some(|conn| Some(if conn == 0 { "model-a" } else { "model-b" }.to_string())),
        ..Script::default()
    };
    let reused = Script { session_id_for: |_| "same-session".into(), ..Script::default() };
    for (script, needle) in [(route_change, "model"), (reused, "session")] {
        let h = Harness::new(script);
        let mut c = h.controller();
        connect_ready(&h, &mut c, 1);
        c.send(1, 1, "a".into()).unwrap();
        assert_eq!(replies(&wait_for(&mut c, WAIT, probe_terminal)).len(), 1);
        c.send(1, 2, "b".into()).unwrap();
        let e = wait_for(&mut c, WAIT, is_disconnected);
        let (_, req, message, uncertain) = failed(&e);
        assert_eq!((req, uncertain), (Some(2), false), "known: nothing was sent: {e:?}");
        assert!(message.contains(needle), "{message}");
        assert_eq!(sends(&h), 1, "the refused turn sent nothing");
        if needle == "model" {
            let e = connect_ready(&h, &mut c, 1);
            assert!(
                matches!(e.last(), Some(ChatEvent::Ready { model: Some(m), .. }) if m == "model-b"),
                "reconnect reports the current route: {e:?}"
            );
        }
    }
}
