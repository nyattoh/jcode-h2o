#![cfg(feature = "native-ui")]
//! Smoke tests for the `h2o-native` executable.
//!
//! Verifies `--help`, `-h`, and invalid CLI argument handling.

use std::process::Command;

#[test]
fn help_prints_usage_and_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_h2o-native"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("usage: h2o-native"), "stdout was: {out}");
    assert!(
        output.stderr.is_empty(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn short_h_prints_usage_and_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_h2o-native"))
        .arg("-h")
        .output()
        .unwrap();
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("usage: h2o-native"), "stdout was: {out}");
    assert!(
        output.stderr.is_empty(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unknown_flag_exits_two_and_prints_usage_on_stderr() {
    let output = Command::new(env!("CARGO_BIN_EXE_h2o-native"))
        .arg("--unknown-flag")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("usage: h2o-native"), "stderr was: {err}");
}
