#![cfg(all(feature = "sdk", feature = "jev"))]
//! Binary smoke tests. No daemon, no network; the child never sees a real key.

use std::{
    fs,
    process::{Command, Output},
    time::{Duration, Instant},
};

mod support;
use support::TestDir;

const SPEC: &str = r#"{"run_id":"run-1","branch_id":"branch-a","owner_session_id":"root",
"session_id":"worker-a","goal":{"purpose":"Verify","constraints":[],"success_conditions":["ok"],
"owned_paths":[],"unfinished":[]},"context_ids":[],"max_corrections":1}"#;

fn monitor(root: &TestDir, extra: &[&str], owner: &str) -> (Output, Duration) {
    let spec = root.path.join("spec.json");
    fs::write(&spec, SPEC).unwrap();
    fs::create_dir_all(root.path.join("sources")).unwrap();
    let socket = root.path.join("no-such.sock");
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_h2o-monitor"))
        .env_remove("TYPESAFE_API_KEY")
        .args(["--socket", socket.to_str().unwrap()])
        .args(["--spec", spec.to_str().unwrap()])
        .args(["--journal", root.path.join("journal").to_str().unwrap()])
        .args(["--sources", root.path.join("sources").to_str().unwrap()])
        .args(["--owner", owner, "--worker", "worker-a"])
        .args(extra)
        .output()
        .unwrap();
    (output, started.elapsed())
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn help_prints_usage_and_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_h2o-monitor"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("usage: h2o-monitor"), "{out}");
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_enable_jev_exits_before_connecting() {
    let root = TestDir::new();
    let (output, _) = monitor(&root, &[], "root");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--enable-jev is required"));
    assert!(!stderr(&output).contains("connect:"));
    assert!(!root.path.join("journal").exists());
}

#[test]
fn owner_mismatch_is_refused() {
    let root = TestDir::new();
    let (output, _) = monitor(&root, &["--enable-jev"], "other-root");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("do not match"));
    assert!(!stderr(&output).contains("connect:"));
}

#[test]
fn nonexistent_socket_fails_fast_without_starting_a_runtime() {
    let root = TestDir::new();
    let (output, elapsed) = monitor(&root, &["--enable-jev"], "root");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("connect:"), "{}", stderr(&output));
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    assert!(!root.path.join("journal").exists());
}
