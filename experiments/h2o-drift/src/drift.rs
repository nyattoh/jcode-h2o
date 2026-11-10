//! Pure branch control; a Jev transport and runtime integration are not yet implemented.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Signal {
    Aligned,
    Misaligned,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decision {
    Continue,
    Correct,
    StopBranch,
    Hold,
}
#[derive(Debug)]
pub struct Branch {
    remaining_corrections: u32,
    correcting: bool,
    stopped: bool,
}
impl Branch {
    pub fn new(max_corrections: u32) -> Self {
        Self {
            remaining_corrections: max_corrections,
            correcting: false,
            stopped: false,
        }
    }
    /// Caller supplies a validated signal after each checkpoint. Unknown never consumes budget.
    pub fn checkpoint(&mut self, signal: Signal) -> Decision {
        if self.stopped {
            return Decision::StopBranch;
        }
        match signal {
            Signal::Unknown => Decision::Hold,
            Signal::Aligned => {
                self.correcting = false;
                Decision::Continue
            }
            Signal::Misaligned => {
                if self.remaining_corrections == 0 {
                    self.stopped = true;
                    Decision::StopBranch
                } else {
                    self.remaining_corrections -= 1;
                    self.correcting = true;
                    Decision::Correct
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn correction_budget_and_unknown() {
        let mut b = Branch::new(2);
        assert_eq!(b.checkpoint(Signal::Misaligned), Decision::Correct);
        assert_eq!(b.checkpoint(Signal::Unknown), Decision::Hold);
        assert_eq!(b.checkpoint(Signal::Misaligned), Decision::Correct);
        assert_eq!(b.checkpoint(Signal::Misaligned), Decision::StopBranch);
        assert_eq!(b.checkpoint(Signal::Aligned), Decision::StopBranch);
    }
    #[test]
    fn successful_correction_continues() {
        let mut b = Branch::new(1);
        assert_eq!(b.checkpoint(Signal::Misaligned), Decision::Correct);
        assert_eq!(b.checkpoint(Signal::Aligned), Decision::Continue);
    }
}
