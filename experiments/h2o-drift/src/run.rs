//! Event loop over `SdkMonitor`. Adds only the stop conditions; all checkpoint logic stays in monitor.
use crate::{
    drift::Decision,
    memory::Memory,
    monitor::{BranchSpec, Judge, SdkMonitor},
};
use jcode_sdk::JcodeClient;
use std::{io, path::Path, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEnd {
    /// StopBranch was decided and its cancel request was acknowledged.
    Stopped,
    /// The SDK transport closed with no control request pending. With the current monitor the
    /// transport-closed Hold's cancel fails on the closed client, so a disconnect normally ends
    /// as `NeedsReconciliation` instead.
    Disconnected,
    /// A control result is uncertain (including after a disconnect); a human must reconcile
    /// before another run.
    NeedsReconciliation,
}

/// Runs until one `RunEnd` holds. Returns it with the journal's checkpoint count.
pub fn run(
    client: JcodeClient,
    spec: BranchSpec,
    sources: Memory,
    journal: &Path,
    judge: &mut impl Judge,
    poll: Duration,
) -> io::Result<(RunEnd, u64)> {
    let observer = client.clone();
    let mut sdk = SdkMonitor::attach(client, spec, sources, journal)?;
    let count = |sdk: &SdkMonitor| sdk.monitor().checkpoint_count();
    let last_stopped = |sdk: &SdkMonitor| -> io::Result<bool> {
        let n = count(sdk);
        Ok(n > 0 && sdk.monitor().record(n)?.decision == Decision::StopBranch)
    };
    loop {
        // Checked first: a disconnect whose Hold/cancel failed is uncertain, not a clean end.
        if sdk.monitor().needs_reconciliation() {
            return Ok((RunEnd::NeedsReconciliation, count(&sdk)));
        }
        if last_stopped(&sdk)? {
            return Ok((RunEnd::Stopped, count(&sdk)));
        }
        if sdk.poll(poll, judge)?.is_none() && observer.is_closed() {
            return Ok((RunEnd::Disconnected, count(&sdk)));
        }
    }
}
