#![cfg(feature = "sdk")]

use h2o_drift_prototype::{
    drift::{Decision, Signal},
    memory::Memory,
    monitor::{
        Action, Assessment, BranchControl, BranchSpec, Goal, Judge, JudgeFailure, JudgementInput,
        Monitor,
    },
};
use jcode_sdk::{ApiEvent, SessionInfo};
use std::{collections::VecDeque, fs, io, path::PathBuf, sync::Mutex};

mod support;
use support::TestDir;

fn spec() -> BranchSpec {
    BranchSpec {
        run_id: "run-1".into(),
        branch_id: "branch-a".into(),
        owner_session_id: "root".into(),
        session_id: "worker-a".into(),
        goal: Goal {
            purpose: "Preserve original bytes".into(),
            constraints: vec!["No external calls".into()],
            success_conditions: vec!["Exact round trip".into()],
            owned_paths: vec!["experiments/h2o-drift".into()],
            unfinished: vec!["Check restoration".into()],
        },
        context_ids: vec!["needed".into()],
        max_corrections: 1,
    }
}

fn member() -> SessionInfo {
    serde_json::from_value(serde_json::json!({"session_id":"worker-a", "parent_session_id":"root", "status":"running"})).unwrap()
}

fn event(call: &str) -> ApiEvent {
    ApiEvent::ToolDone {
        session_id: "worker-a".into(),
        call_id: call.into(),
        name: "test".into(),
        output: "Observed fixture result".into(),
        error: None,
    }
}

struct Script {
    answers: VecDeque<Result<Assessment, JudgeFailure>>,
    inputs: Vec<JudgementInput>,
}

impl Script {
    fn signals(signals: &[Signal]) -> Self {
        Self {
            answers: signals
                .iter()
                .map(|signal| {
                    Ok(Assessment {
                        signal: *signal,
                        reason: "fixture evidence".into(),
                    })
                })
                .collect(),
            inputs: vec![],
        }
    }
}

impl Judge for Script {
    fn assess(&mut self, input: &JudgementInput) -> Result<Assessment, JudgeFailure> {
        self.inputs.push(input.clone());
        self.answers
            .pop_front()
            .expect("unexpected judge invocation")
    }
}

struct Control {
    calls: Mutex<Vec<(String, Action)>>,
    fail: bool,
    journal: PathBuf,
}

impl Control {
    fn new(root: &TestDir) -> Self {
        Self {
            calls: Mutex::new(vec![]),
            fail: false,
            journal: root.path.join("journal"),
        }
    }
}

impl BranchControl for Control {
    fn apply(&self, session_id: &str, action: &Action) -> io::Result<()> {
        assert!(
            fs::read_dir(&self.journal).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("event-")),
            "checkpoint must precede control"
        );
        self.calls
            .lock()
            .unwrap()
            .push((session_id.into(), action.clone()));
        if self.fail {
            Err(io::Error::other("lost acknowledgement"))
        } else {
            Ok(())
        }
    }
}

fn open(root: &TestDir, spec: BranchSpec) -> Monitor {
    let mut sources = Memory::open(root.path.join("sources")).unwrap();
    if sources.get("needed").is_err() {
        sources.insert("needed", b"required context\0\xff").unwrap();
        sources.insert("unrelated", b"do not forward").unwrap();
    }
    Monitor::open(spec, &member(), sources, root.path.join("journal")).unwrap()
}

#[test]
fn progress_replan_recovery_and_lifetime_stop_preserve_evidence() {
    let root = TestDir::new();
    let mut monitor = open(&root, spec());
    let control = Control::new(&root);
    let mut judge = Script::signals(&[
        Signal::Aligned,
        Signal::Misaligned,
        Signal::Aligned,
        Signal::Misaligned,
    ]);
    for (call, expected) in [
        ("one", Decision::Continue),
        ("two", Decision::Correct),
        ("three", Decision::Continue),
        ("four", Decision::StopBranch),
    ] {
        assert_eq!(
            monitor
                .on_event(&event(call), &mut judge, &control)
                .unwrap()
                .unwrap()
                .decision,
            expected
        );
    }
    let calls = control.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|(session, _)| session == "worker-a"));
    let Action::Replan { message } = &calls[0].1 else {
        panic!("expected replan")
    };
    assert!(message.contains("Preserve original bytes"));
    assert!(message.contains("No external calls"));
    assert!(message.contains("needed"));
    assert!(!message.contains("unrelated"));
    assert_eq!(calls[1].1, Action::StopBranch);
    assert_eq!(monitor.spec(), &spec());
    assert_eq!(
        monitor.record(4).unwrap().input.observation.output,
        "Observed fixture result"
    );
    assert!(
        monitor
            .on_event(&event("late"), &mut judge, &control)
            .unwrap()
            .is_none()
    );
    assert_eq!(judge.inputs.len(), 4);
    assert_eq!(judge.inputs[0].context.len(), 1);
    assert_eq!(judge.inputs[0].context[0].bytes, b"required context\0\xff");
}

#[test]
fn judge_failures_hold_without_consuming_correction_budget() {
    let root = TestDir::new();
    let mut monitor = open(&root, spec());
    let control = Control::new(&root);
    let mut judge = Script::signals(&[Signal::Misaligned, Signal::Misaligned]);
    for failure in [
        JudgeFailure::Timeout,
        JudgeFailure::Unavailable,
        JudgeFailure::Malformed,
    ]
    .into_iter()
    .rev()
    {
        judge.answers.push_front(Err(failure));
    }
    for call in ["timeout", "unavailable", "malformed"] {
        let checkpoint = monitor
            .on_event(&event(call), &mut judge, &control)
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.decision, Decision::Hold);
        assert_eq!(checkpoint.action, Action::Hold);
    }
    assert_eq!(
        monitor
            .on_event(&event("correct"), &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::Correct
    );
    assert_eq!(
        monitor
            .on_event(&event("stop"), &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::StopBranch
    );
    assert_eq!(monitor.checkpoint_count(), 5);
}

#[test]
fn absent_context_and_empty_observations_bypass_the_judge_and_hold() {
    let root = TestDir::new();
    let mut monitor = open(&root, spec());
    let control = Control::new(&root);
    let mut judge = Script::signals(&[]);
    let empty = ApiEvent::ToolDone {
        session_id: "worker-a".into(),
        call_id: "empty".into(),
        name: "test".into(),
        output: String::new(),
        error: None,
    };
    assert_eq!(
        monitor
            .on_event(&empty, &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::Hold
    );
    let blank = ApiEvent::ToolDone {
        session_id: "worker-a".into(),
        call_id: "blank-error".into(),
        name: "test".into(),
        output: " \n".into(),
        error: Some(String::new()),
    };
    assert_eq!(
        monitor
            .on_event(&blank, &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::Hold
    );
    fs::remove_file(root.path.join("sources/needed")).unwrap();
    assert_eq!(
        monitor
            .on_event(&event("missing"), &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::Hold
    );
    assert!(judge.inputs.is_empty());
}

#[test]
fn duplicate_foreign_and_reasoning_events_cannot_spend_budget_or_trigger_control() {
    let root = TestDir::new();
    let mut monitor = open(&root, spec());
    let control = Control::new(&root);
    let mut judge = Script::signals(&[Signal::Misaligned]);
    let mut foreign = event("foreign");
    if let ApiEvent::ToolDone { session_id, .. } = &mut foreign {
        *session_id = "worker-b".into();
    }
    assert!(
        monitor
            .on_event(&foreign, &mut judge, &control)
            .unwrap()
            .is_none()
    );
    assert!(
        monitor
            .on_event(
                &ApiEvent::ReasoningDelta {
                    session_id: "worker-a".into(),
                    text: "not evidence".into()
                },
                &mut judge,
                &control
            )
            .unwrap()
            .is_none()
    );
    monitor
        .on_event(&event("one"), &mut judge, &control)
        .unwrap();
    assert!(
        monitor
            .on_event(&event("one"), &mut judge, &control)
            .unwrap()
            .is_none()
    );
    assert_eq!(judge.inputs.len(), 1);
    assert_eq!(control.calls.lock().unwrap().len(), 1);
}

#[test]
fn restart_replays_budget_without_repeating_control_and_rejects_goal_changes() {
    let root = TestDir::new();
    let control = Control::new(&root);
    let mut judge = Script::signals(&[Signal::Misaligned, Signal::Misaligned]);
    open(&root, spec())
        .on_event(&event("one"), &mut judge, &control)
        .unwrap();
    let mut reopened = open(&root, spec());
    assert_eq!(control.calls.lock().unwrap().len(), 1);
    assert!(
        reopened
            .on_event(&event("one"), &mut judge, &control)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .on_event(&event("two"), &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::StopBranch
    );
    let mut changed = spec();
    changed.goal.purpose = "different purpose".into();
    let sources = Memory::open(root.path.join("sources")).unwrap();
    assert!(Monitor::open(changed, &member(), sources, root.path.join("journal")).is_err());
}

#[test]
fn missing_or_negative_receipts_require_reconciliation_without_resending() {
    for missing in [false, true] {
        let root = TestDir::new();
        let mut control = Control::new(&root);
        control.fail = !missing;
        let mut judge = Script::signals(&[Signal::Misaligned]);
        let mut monitor = open(&root, spec());
        monitor
            .on_event(&event("one"), &mut judge, &control)
            .unwrap();
        if missing {
            fs::remove_file(root.path.join("journal/receipt-00000000000000000001")).unwrap();
        }
        drop(monitor);
        let mut reopened = open(&root, spec());
        assert!(reopened.needs_reconciliation());
        assert_eq!(
            reopened
                .on_event(&event("two"), &mut judge, &control)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(control.calls.lock().unwrap().len(), 1);
        assert_eq!(reopened.record(1).unwrap().input.goal, spec().goal);
    }
}

#[test]
fn wrong_owner_is_rejected_before_creating_a_journal() {
    let root = TestDir::new();
    let mut wrong = member();
    wrong.parent_session_id = Some("someone-else".into());
    let result = Monitor::open(
        spec(),
        &wrong,
        Memory::open(root.path.join("sources")).unwrap(),
        root.path.join("journal"),
    );
    assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::PermissionDenied));
    assert!(!root.path.join("journal").exists());
}

#[test]
fn checkpoint_gaps_and_malformed_records_are_not_silently_replayed() {
    for corrupt in [false, true] {
        let root = TestDir::new();
        let control = Control::new(&root);
        let mut judge = Script::signals(&[Signal::Aligned, Signal::Aligned]);
        let mut monitor = open(&root, spec());
        monitor
            .on_event(&event("one"), &mut judge, &control)
            .unwrap();
        monitor
            .on_event(&event("two"), &mut judge, &control)
            .unwrap();
        drop(monitor);
        let path = root.path.join("journal/event-00000000000000000001");
        if corrupt {
            fs::write(path, b"invalid JSON").unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
        let result = Monitor::open(
            spec(),
            &member(),
            Memory::open(root.path.join("sources")).unwrap(),
            root.path.join("journal"),
        );
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidData));
        assert!(control.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn an_assessment_without_a_reason_cannot_spend_correction_budget() {
    let root = TestDir::new();
    let mut monitor = open(&root, spec());
    let control = Control::new(&root);
    let mut judge = Script::signals(&[Signal::Misaligned]);
    judge.answers.push_front(Ok(Assessment {
        signal: Signal::Misaligned,
        reason: String::new(),
    }));
    assert_eq!(
        monitor
            .on_event(&event("empty-reason"), &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::Hold
    );
    assert_eq!(
        monitor
            .on_event(&event("evidence"), &mut judge, &control)
            .unwrap()
            .unwrap()
            .decision,
        Decision::Correct
    );
}

#[test]
fn concurrent_monitor_handles_cannot_issue_two_controls_for_the_same_sequence() {
    let root = TestDir::new();
    let mut first = open(&root, spec());
    let mut stale = open(&root, spec());
    let control = Control::new(&root);
    let mut judge = Script::signals(&[Signal::Misaligned, Signal::Misaligned]);
    first
        .on_event(&event("first"), &mut judge, &control)
        .unwrap();
    assert_eq!(
        stale
            .on_event(&event("stale"), &mut judge, &control)
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(control.calls.lock().unwrap().len(), 1);
    assert_eq!(first.record(1).unwrap().input.observation.call_id, "first");
}
