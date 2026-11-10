use h2o_drift_prototype::drift::{Branch, Decision, Signal};

const SIGNALS: [Signal; 3] = [Signal::Aligned, Signal::Misaligned, Signal::Unknown];

fn expected(prefix: &[Signal], budget: usize) -> Decision {
    let deviations = prefix
        .iter()
        .filter(|signal| **signal == Signal::Misaligned)
        .count();
    if deviations > budget {
        return Decision::StopBranch;
    }
    match prefix.last().unwrap() {
        Signal::Aligned => Decision::Continue,
        Signal::Unknown => Decision::Hold,
        Signal::Misaligned => Decision::Correct,
    }
}

#[test]
fn all_nine_checkpoint_histories_obey_the_lifetime_budget_contract() {
    const LENGTH: usize = 9;
    for encoded in 0..3usize.pow(LENGTH as u32) {
        let mut code = encoded;
        let history: Vec<_> = (0..LENGTH)
            .map(|_| {
                let signal = SIGNALS[code % 3];
                code /= 3;
                signal
            })
            .collect();
        for budget in 0..=3 {
            let mut branch = Branch::new(budget as u32);
            for end in 1..=LENGTH {
                assert_eq!(
                    branch.checkpoint(history[end - 1]),
                    expected(&history[..end], budget),
                    "history={encoded}, prefix={end}, budget={budget}"
                );
            }
        }
    }
}

#[test]
fn all_seven_checkpoint_interleavings_keep_two_branch_budgets_independent() {
    const LENGTH: usize = 7;
    for encoded in 0..6usize.pow(LENGTH as u32) {
        let mut code = encoded;
        let mut branches = [Branch::new(1), Branch::new(2)];
        let mut histories = [vec![], vec![]];
        for _ in 0..LENGTH {
            let choice = code % 6;
            code /= 6;
            let index = choice / 3;
            let signal = SIGNALS[choice % 3];
            histories[index].push(signal);
            assert_eq!(
                branches[index].checkpoint(signal),
                expected(&histories[index], index + 1),
                "interleaving={encoded}, branch={index}"
            );
        }
    }
}
