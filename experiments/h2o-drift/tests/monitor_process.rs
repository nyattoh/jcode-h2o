#![cfg(feature = "sdk")]

use h2o_drift_prototype::{
    drift::{Decision, Signal},
    memory::Memory,
    monitor::{
        Action, Assessment, BranchControl, BranchSpec, Goal, Judge, JudgementInput, Monitor,
    },
};
use jcode_sdk::{ApiEvent, SessionInfo};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

mod support;
use support::TestDir;

const ORIGINAL: &[u8] = b"original context\0\xff";

fn spec() -> BranchSpec {
    BranchSpec {
        run_id: "process-run".into(),
        branch_id: "process-branch".into(),
        owner_session_id: "root".into(),
        session_id: "worker".into(),
        goal: Goal {
            purpose: "Preserve source and correction budget across monitor restart".into(),
            constraints: vec!["Local mocks only".into()],
            success_conditions: vec!["No repeated control".into()],
            owned_paths: vec!["synthetic-fixture".into()],
            unfinished: vec!["Complete source check".into()],
        },
        context_ids: vec!["source".into()],
        max_corrections: 1,
    }
}

fn member() -> SessionInfo {
    serde_json::from_value(
        serde_json::json!({"session_id":"worker", "parent_session_id":"root", "status":"running"}),
    )
    .unwrap()
}

fn open(root: &Path) -> Monitor {
    let mut source = Memory::open(root.join("sources")).unwrap();
    if source.get("source").is_err() {
        source.insert("source", ORIGINAL).unwrap();
    }
    Monitor::open(spec(), &member(), source, root.join("journal")).unwrap()
}

fn event(id: &str) -> ApiEvent {
    ApiEvent::ToolDone {
        session_id: "worker".into(),
        call_id: id.into(),
        name: "synthetic-check".into(),
        output: "observed deviation".into(),
        error: None,
    }
}

struct Misaligned;
impl Judge for Misaligned {
    fn assess(
        &mut self,
        input: &JudgementInput,
    ) -> Result<Assessment, h2o_drift_prototype::monitor::JudgeFailure> {
        assert_eq!(input.context[0].bytes, ORIGINAL);
        Ok(Assessment {
            signal: Signal::Misaligned,
            reason: "synthetic drift".into(),
        })
    }
}

struct NoJudge;
impl Judge for NoJudge {
    fn assess(
        &mut self,
        _: &JudgementInput,
    ) -> Result<Assessment, h2o_drift_prototype::monitor::JudgeFailure> {
        panic!("replay or stopped state must not call the judge")
    }
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "checkpoint not reached: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

struct Process(Child);
impl Process {
    fn start(root: &Path, mode: &str, label: &str) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "subprocess_entry"])
                .env("H2O_MONITOR_TEST_ROOT", root)
                .env("H2O_MONITOR_TEST_MODE", mode)
                .env("H2O_MONITOR_TEST_LABEL", label)
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "monitor child failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "monitor child did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn kill(&mut self) {
        self.0.kill().unwrap();
        assert!(!self.0.wait().unwrap().success());
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct FileControl<'a> {
    root: &'a Path,
    mode: &'a str,
    label: &'a str,
}
impl BranchControl for FileControl<'_> {
    fn apply(&self, session_id: &str, action: &Action) -> io::Result<()> {
        assert_eq!(session_id, "worker");
        if self.mode == "before-control" {
            fs::write(self.root.join("ready"), [])?;
            wait_for(&self.root.join("never-released"));
        }
        let path = self.root.join(format!("effect-{}", self.label));
        let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
        file.write_all(&serde_json::to_vec(action).unwrap())?;
        file.sync_all()?;
        if self.mode == "after-control" {
            fs::write(self.root.join("ready"), [])?;
            wait_for(&self.root.join("never-released"));
        }
        Ok(())
    }
}

fn effects(root: &Path) -> usize {
    fs::read_dir(root)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("effect-")
        })
        .count()
}

#[test]
fn interrupted_control_is_not_reissued_by_any_restarted_monitor() {
    for mode in ["before-control", "after-control"] {
        let root = TestDir::new();
        let mut writer = Process::start(&root.path, mode, "original");
        wait_for(&root.path.join("ready"));
        writer.kill();
        for label in ["restart-a", "restart-b"] {
            Process::start(&root.path, "verify-uncertain", label).finish();
        }
        assert_eq!(effects(&root.path), usize::from(mode == "after-control"));
        let monitor = open(&root.path);
        assert!(monitor.needs_reconciliation());
        assert_eq!(monitor.record(1).unwrap().input.goal, spec().goal);
        assert_eq!(monitor.record(1).unwrap().input.observation.call_id, "one");
        assert_eq!(monitor.record(1).unwrap().input.context[0].bytes, ORIGINAL);
    }
}

#[test]
fn acknowledged_budget_and_terminal_stop_survive_real_process_restarts() {
    let root = TestDir::new();
    let mut writer = Process::start(&root.path, "after-receipt", "original");
    wait_for(&root.path.join("ready"));
    writer.kill();
    Process::start(&root.path, "spend-remainder", "stop").finish();
    Process::start(&root.path, "verify-stopped", "reader").finish();
    assert_eq!(effects(&root.path), 2);
    let monitor = open(&root.path);
    assert_eq!(monitor.checkpoint_count(), 2);
    assert_eq!(monitor.record(1).unwrap().decision, Decision::Correct);
    assert_eq!(monitor.record(2).unwrap().decision, Decision::StopBranch);
    assert!(!monitor.needs_reconciliation());
}

#[test]
fn concurrent_monitor_processes_reserve_one_checkpoint_before_control() {
    let root = TestDir::new();
    drop(open(&root.path));
    let mut first = Process::start(&root.path, "race", "a");
    let mut second = Process::start(&root.path, "race", "b");
    wait_for(&root.path.join("ready-a"));
    wait_for(&root.path.join("ready-b"));
    fs::write(root.path.join("go"), []).unwrap();
    first.finish();
    second.finish();
    let results = [
        fs::read(root.path.join("result-a")).unwrap(),
        fs::read(root.path.join("result-b")).unwrap(),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.as_slice() == b"applied")
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.as_slice() == b"collision")
            .count(),
        1
    );
    assert_eq!(effects(&root.path), 1);
    assert_eq!(open(&root.path).checkpoint_count(), 1);
}

#[test]
#[ignore = "subprocess fixture; parent tests invoke it explicitly"]
fn subprocess_entry() {
    let root = std::path::PathBuf::from(std::env::var_os("H2O_MONITOR_TEST_ROOT").unwrap());
    let mode = std::env::var("H2O_MONITOR_TEST_MODE").unwrap();
    let label = std::env::var("H2O_MONITOR_TEST_LABEL").unwrap();
    let mut monitor = open(&root);
    let control = FileControl {
        root: &root,
        mode: &mode,
        label: &label,
    };
    match mode.as_str() {
        "before-control" | "after-control" | "after-receipt" => {
            assert_eq!(
                monitor
                    .on_event(&event("one"), &mut Misaligned, &control)
                    .unwrap()
                    .unwrap()
                    .decision,
                Decision::Correct
            );
            fs::write(root.join("ready"), []).unwrap();
            wait_for(&root.join("never-released"));
        }
        "verify-uncertain" => {
            assert!(monitor.needs_reconciliation());
            assert_eq!(monitor.checkpoint_count(), 1);
            for id in ["one", "two"] {
                assert_eq!(
                    monitor
                        .on_event(&event(id), &mut NoJudge, &control)
                        .unwrap_err()
                        .kind(),
                    io::ErrorKind::WouldBlock
                );
            }
        }
        "spend-remainder" => {
            assert!(!monitor.needs_reconciliation());
            assert!(
                monitor
                    .on_event(&event("one"), &mut NoJudge, &control)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                monitor
                    .on_event(&event("two"), &mut Misaligned, &control)
                    .unwrap()
                    .unwrap()
                    .decision,
                Decision::StopBranch
            );
        }
        "verify-stopped" => {
            assert_eq!(monitor.checkpoint_count(), 2);
            assert!(
                monitor
                    .on_event(&event("three"), &mut NoJudge, &control)
                    .unwrap()
                    .is_none()
            );
        }
        "race" => {
            fs::write(root.join(format!("ready-{label}")), []).unwrap();
            wait_for(&root.join("go"));
            let result = match monitor.on_event(&event(&label), &mut Misaligned, &control) {
                Ok(Some(_)) => "applied",
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => "collision",
                other => panic!("unexpected result: {other:?}"),
            };
            fs::write(root.join(format!("result-{label}")), result).unwrap();
        }
        _ => panic!("unknown monitor subprocess mode"),
    }
}
