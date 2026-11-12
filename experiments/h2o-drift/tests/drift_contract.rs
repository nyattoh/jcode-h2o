use h2o_drift_prototype::drift::{Branch, Decision, Signal};

#[test]
fn zero_budget_still_allows_progress_and_holds_unknown_before_stopping() {
    let mut branch = Branch::new(0);
    assert_eq!(branch.checkpoint(Signal::Aligned), Decision::Continue);
    assert_eq!(branch.checkpoint(Signal::Unknown), Decision::Hold);
    assert_eq!(branch.checkpoint(Signal::Misaligned), Decision::StopBranch);
}

#[test]
fn unknown_never_spends_corrections_even_after_budget_is_exhausted() {
    let mut branch = Branch::new(1);
    for _ in 0..20 {
        assert_eq!(branch.checkpoint(Signal::Unknown), Decision::Hold);
    }
    assert_eq!(branch.checkpoint(Signal::Misaligned), Decision::Correct);
    for _ in 0..20 {
        assert_eq!(branch.checkpoint(Signal::Unknown), Decision::Hold);
    }
    assert_eq!(branch.checkpoint(Signal::Misaligned), Decision::StopBranch);
}

#[test]
fn successful_recovery_never_replenishes_branch_lifetime_budget() {
    let mut branch = Branch::new(2);
    for _ in 0..2 {
        assert_eq!(branch.checkpoint(Signal::Misaligned), Decision::Correct);
        assert_eq!(branch.checkpoint(Signal::Aligned), Decision::Continue);
    }
    assert_eq!(branch.checkpoint(Signal::Misaligned), Decision::StopBranch);
}

#[test]
fn stopped_branch_remains_stopped_for_every_signal() {
    let mut branch = Branch::new(0);
    assert_eq!(branch.checkpoint(Signal::Misaligned), Decision::StopBranch);
    for signal in [
        Signal::Unknown,
        Signal::Aligned,
        Signal::Misaligned,
        Signal::Aligned,
    ] {
        assert_eq!(branch.checkpoint(signal), Decision::StopBranch);
    }
}

#[test]
fn stopping_one_branch_does_not_stop_or_spend_another_branch_budget() {
    let mut stopped = Branch::new(0);
    let mut active = Branch::new(1);
    assert_eq!(stopped.checkpoint(Signal::Misaligned), Decision::StopBranch);
    assert_eq!(active.checkpoint(Signal::Aligned), Decision::Continue);
    assert_eq!(active.checkpoint(Signal::Unknown), Decision::Hold);
    assert_eq!(active.checkpoint(Signal::Misaligned), Decision::Correct);
    assert_eq!(active.checkpoint(Signal::Aligned), Decision::Continue);
    assert_eq!(stopped.checkpoint(Signal::Aligned), Decision::StopBranch);
}
