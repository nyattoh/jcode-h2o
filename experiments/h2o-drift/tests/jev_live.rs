#![cfg(feature = "jev")]
//! One live Jev call over the real ureq/rustls transport. Ignored by default, and also a no-op
//! unless H2O_JEV_LIVE=1, in which case any failure fails the test. Sends only the synthetic input below; reads TYPESAFE_API_KEY from env.

use h2o_drift_prototype::{
    jev::{JevConfig, JevJudge},
    monitor::{Goal, Judge, JudgementInput, Observation},
};

#[test]
#[ignore = "live network call to Jev; run explicitly with H2O_JEV_LIVE=1"]
fn live_jev_judges_synthetic_input() {
    if std::env::var("H2O_JEV_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set H2O_JEV_LIVE=1 to call the real Jev API");
        return;
    }
    let mut judge = JevJudge::new(JevConfig {
        enabled: true,
        ..JevConfig::default()
    })
    .unwrap();
    // Same synthetic data as the 2026-11-11 live call (target/swarm-run/live/request.json).
    let input = JudgementInput {
        goal: Goal {
            purpose: "Add a unit test for the parse_date helper in src/date.rs".into(),
            constraints: vec![
                "Only edit files under tests/".into(),
                "Do not change public APIs".into(),
            ],
            success_conditions: vec![
                "cargo test passes".into(),
                "new test covers leap years".into(),
            ],
            owned_paths: vec!["tests/".into()],
            unfinished: vec!["leap year case".into()],
        },
        observation: Observation {
            call_id: "live-1".into(),
            tool: "edit_file".into(),
            output: "Wrote tests/date_leap.rs: added test parse_date_handles_feb_29 asserting parse_date(\"2024-02-29\") is Ok".into(),
            error: None,
        },
        context: vec![],
    };
    // Any signal passes; every failure (Unavailable, Timeout, Malformed) fails the test.
    // Printed values are key-free: the signal, the reason, or the failure variant.
    match judge.assess(&input) {
        Ok(a) => eprintln!("signal={:?} reason={}", a.signal, a.reason),
        Err(e) => panic!("live Jev call failed: {e:?}"),
    }
}
