//! Cooperative SDK checkpoint monitoring, independent of provider/model/effort routing.
//! Observes completed tools; it is not a pre-execution permission boundary.
use crate::{
    drift::{Branch, Decision, Signal},
    memory::Memory,
};
use jcode_sdk::{ApiEvent, EventStream, JcodeClient, SessionInfo};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, io, path::Path, time::Duration};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub purpose: String,
    pub constraints: Vec<String>,
    pub success_conditions: Vec<String>,
    pub owned_paths: Vec<String>,
    pub unfinished: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchSpec {
    pub run_id: String,
    pub branch_id: String,
    pub owner_session_id: String,
    pub session_id: String,
    pub goal: Goal,
    pub context_ids: Vec<String>,
    pub max_corrections: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub call_id: String,
    pub tool: String,
    pub output: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JudgementInput {
    pub goal: Goal,
    pub observation: Observation,
    pub context: Vec<Source>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    pub signal: Signal,
    pub reason: String,
}

impl Assessment {
    fn unknown(reason: &str) -> Self {
        Self {
            signal: Signal::Unknown,
            reason: reason.into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum JudgeFailure {
    Timeout,
    Unavailable,
    Malformed,
}

/// This stage supplies local mocks only. Implementations must bound their own runtime.
pub trait Judge {
    fn assess(&mut self, input: &JudgementInput) -> Result<Assessment, JudgeFailure>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    Continue,
    Replan { message: String },
    Hold,
    StopBranch,
}

pub trait BranchControl {
    fn apply(&self, session_id: &str, action: &Action) -> io::Result<()>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub sequence: u64,
    pub input: JudgementInput,
    pub assessment: Assessment,
    pub decision: Decision,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub acknowledged: bool,
}

pub struct Monitor {
    spec: BranchSpec,
    branch: Branch,
    sources: Memory,
    journal: Memory,
    sequence: u64,
    seen: HashMap<String, u64>,
    stopped: bool,
    uncertain: bool,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn validate_member(spec: &BranchSpec, member: &SessionInfo) -> io::Result<()> {
    if member.session_id != spec.session_id
        || member.parent_session_id.as_deref() != Some(spec.owner_session_id.as_str())
        || spec.owner_session_id == spec.session_id
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "worker ownership does not match",
        ));
    }
    Ok(())
}

fn key(prefix: &str, sequence: u64) -> String {
    format!("{prefix}-{sequence:020}")
}

fn read_json<T: serde::de::DeserializeOwned>(store: &Memory, id: &str) -> io::Result<T> {
    serde_json::from_slice(&store.get(id)?).map_err(|_| invalid("invalid monitor record"))
}

fn write_json(store: &mut Memory, id: &str, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| invalid("cannot encode monitor record"))?;
    store.insert(id, &bytes)
}

fn journal_sequences(root: &Path) -> io::Result<Vec<u64>> {
    let mut events = Vec::new();
    let mut receipts = Vec::new();
    for entry in fs::read_dir(root)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        for (prefix, sequences) in [("event", &mut events), ("receipt", &mut receipts)] {
            if let Some(suffix) = name.strip_prefix(&format!("{prefix}-")) {
                let sequence = suffix
                    .parse::<u64>()
                    .map_err(|_| invalid("invalid journal record name"))?;
                if sequence == 0 || name != key(prefix, sequence) {
                    return Err(invalid("noncanonical journal record name"));
                }
                sequences.push(sequence);
            }
        }
    }
    events.sort_unstable();
    if receipts
        .iter()
        .any(|sequence| events.binary_search(sequence).is_err())
    {
        return Err(invalid("receipt has no checkpoint"));
    }
    Ok(events)
}

impl Monitor {
    /// `journal_root` must be an application-owned directory dedicated to this branch and run.
    pub fn open(
        spec: BranchSpec,
        member: &SessionInfo,
        sources: Memory,
        journal_root: impl AsRef<Path>,
    ) -> io::Result<Self> {
        validate_member(&spec, member)?;
        if [
            &spec.run_id,
            &spec.branch_id,
            &spec.session_id,
            &spec.owner_session_id,
            &spec.goal.purpose,
        ]
        .iter()
        .any(|s| s.trim().is_empty())
            || spec.goal.success_conditions.is_empty()
            || spec
                .goal
                .success_conditions
                .iter()
                .any(|s| s.trim().is_empty())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "incomplete branch contract",
            ));
        }
        let root = journal_root.as_ref();
        let mut journal = Memory::open(root)?;
        let sequences = journal_sequences(root)?;
        match read_json::<BranchSpec>(&journal, "spec") {
            Ok(saved) if saved != spec => {
                return Err(invalid("branch contract changed; use a new journal"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !sequences.is_empty() {
                    return Err(invalid("checkpoint history has no branch contract"));
                }
                write_json(&mut journal, "spec", &spec)?
            }
            Err(error) => return Err(error),
        }
        let mut branch = Branch::new(spec.max_corrections);
        let mut seen = HashMap::new();
        let mut stopped = false;
        let mut uncertain = false;
        for (index, sequence) in sequences.iter().enumerate() {
            if *sequence != index as u64 + 1 {
                return Err(invalid("checkpoint gap"));
            }
            let record: Checkpoint = read_json(&journal, &key("event", *sequence))?;
            if record.sequence != *sequence
                || record.input.goal != spec.goal
                || branch.checkpoint(record.assessment.signal) != record.decision
                || seen
                    .insert(record.input.observation.call_id.clone(), *sequence)
                    .is_some()
            {
                return Err(invalid("inconsistent checkpoint history"));
            }
            stopped = record.decision == Decision::StopBranch;
            match read_json::<Receipt>(&journal, &key("receipt", *sequence)) {
                Ok(receipt) => uncertain |= !receipt.acknowledged,
                Err(error) if error.kind() == io::ErrorKind::NotFound => uncertain = true,
                Err(error) => return Err(error),
            }
        }
        Ok(Self {
            spec,
            branch,
            sources,
            journal,
            sequence: sequences.len() as u64,
            seen,
            stopped,
            uncertain,
        })
    }

    pub fn spec(&self) -> &BranchSpec {
        &self.spec
    }
    pub fn needs_reconciliation(&self) -> bool {
        self.uncertain
    }
    pub fn checkpoint_count(&self) -> u64 {
        self.sequence
    }
    pub fn record(&self, sequence: u64) -> io::Result<Checkpoint> {
        read_json(&self.journal, &key("event", sequence))
    }

    pub fn on_event(
        &mut self,
        event: &ApiEvent,
        judge: &mut impl Judge,
        control: &impl BranchControl,
    ) -> io::Result<Option<Checkpoint>> {
        match event {
            ApiEvent::ToolDone {
                session_id,
                call_id,
                name,
                output,
                error,
            } if session_id == &self.spec.session_id => self.checkpoint(
                Observation {
                    call_id: call_id.clone(),
                    tool: name.clone(),
                    output: output.clone(),
                    error: error.clone(),
                },
                None,
                judge,
                control,
            ),
            _ => Ok(None),
        }
    }

    fn checkpoint(
        &mut self,
        observation: Observation,
        forced: Option<Assessment>,
        judge: &mut impl Judge,
        control: &impl BranchControl,
    ) -> io::Result<Option<Checkpoint>> {
        if self.uncertain {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "control result uncertain; reconcile before continuing",
            ));
        }
        if self.stopped || self.seen.contains_key(&observation.call_id) {
            return Ok(None);
        }
        if observation.call_id.trim().is_empty() {
            return Err(invalid("checkpoint requires a call ID"));
        }
        let mut context = Vec::new();
        let mut source_missing = false;
        for id in &self.spec.context_ids {
            match self.sources.get(id) {
                Ok(bytes) => context.push(Source {
                    id: id.clone(),
                    bytes,
                }),
                Err(_) => source_missing = true,
            }
        }
        let input = JudgementInput {
            goal: self.spec.goal.clone(),
            observation,
            context,
        };
        let assessment = if let Some(forced) = forced {
            forced
        } else if source_missing {
            Assessment::unknown("requested source unavailable")
        } else if input.observation.tool.trim().is_empty()
            || (input.observation.output.trim().is_empty()
                && input
                    .observation
                    .error
                    .as_ref()
                    .is_none_or(|error| error.trim().is_empty()))
        {
            Assessment::unknown("insufficient observed evidence")
        } else {
            match judge.assess(&input) {
                Ok(answer) if !answer.reason.trim().is_empty() => answer,
                Ok(_) => Assessment::unknown("missing assessment evidence"),
                Err(JudgeFailure::Timeout) => Assessment::unknown("judge timeout"),
                Err(JudgeFailure::Unavailable) => Assessment::unknown("judge unavailable"),
                Err(JudgeFailure::Malformed) => Assessment::unknown("malformed judge response"),
            }
        };
        let mut next_branch = self.branch.clone();
        let decision = next_branch.checkpoint(assessment.signal);
        let action = match decision {
            Decision::Continue => Action::Continue,
            Decision::Hold => Action::Hold,
            Decision::StopBranch => Action::StopBranch,
            Decision::Correct => Action::Replan {
                message: format!(
                    "Replan within the original goal and constraints. Restore the supplied context before proceeding. Do not change provider, model or effort on the monitor's authority.\nAssessment: {}\n{}",
                    assessment.reason,
                    serde_json::to_string(&input)
                        .map_err(|_| invalid("cannot encode recovery context"))?
                ),
            },
        };
        let record = Checkpoint {
            sequence: self.sequence + 1,
            input,
            assessment,
            decision,
            action,
        };
        write_json(&mut self.journal, &key("event", record.sequence), &record)?;
        self.branch = next_branch;
        self.sequence = record.sequence;
        self.seen
            .insert(record.input.observation.call_id.clone(), record.sequence);
        self.stopped = decision == Decision::StopBranch;
        // Until the receipt is durable, the request may have been applied even when its reply was lost.
        self.uncertain = true;
        let acknowledged = record.action == Action::Continue
            || control.apply(&self.spec.session_id, &record.action).is_ok();
        write_json(
            &mut self.journal,
            &key("receipt", record.sequence),
            &Receipt { acknowledged },
        )?;
        self.uncertain = !acknowledged;
        Ok(Some(record))
    }
}

struct OwnedSdkControl<'a> {
    client: &'a JcodeClient,
    spec: &'a BranchSpec,
}

impl BranchControl for OwnedSdkControl<'_> {
    fn apply(&self, session_id: &str, action: &Action) -> io::Result<()> {
        let member = self
            .client
            .attach_session(session_id)
            .map_err(|_| io::Error::other("cannot revalidate worker ownership"))?;
        validate_member(self.spec, &member)?;
        match action {
            Action::Continue => Ok(()),
            Action::Replan { message } => self
                .client
                .soft_interrupt(session_id, message, false)
                .map_err(|_| io::Error::other("replan request not acknowledged")),
            Action::Hold | Action::StopBranch => self
                .client
                .cancel(session_id)
                .map_err(|_| io::Error::other("cancel request not acknowledged")),
        }
    }
}

pub struct SdkMonitor {
    client: JcodeClient,
    events: EventStream,
    monitor: Monitor,
    disconnect_recorded: bool,
}

impl SdkMonitor {
    pub fn attach(
        client: JcodeClient,
        spec: BranchSpec,
        sources: Memory,
        journal_root: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let events = client.events(Some(&spec.session_id));
        let member = client
            .attach_session(&spec.session_id)
            .map_err(|_| io::Error::other("cannot attach worker"))?;
        let monitor = Monitor::open(spec, &member, sources, journal_root)?;
        Ok(Self {
            client,
            events,
            monitor,
            disconnect_recorded: false,
        })
    }

    pub fn monitor(&self) -> &Monitor {
        &self.monitor
    }

    /// Idle timeout is not a judge failure. A closed transport records unknown exactly once.
    pub fn poll(
        &mut self,
        timeout: Duration,
        judge: &mut impl Judge,
    ) -> io::Result<Option<Checkpoint>> {
        let spec = self.monitor.spec.clone();
        let control = OwnedSdkControl {
            client: &self.client,
            spec: &spec,
        };
        if let Some(event) = self.events.next_timeout(timeout) {
            return self.monitor.on_event(&event, judge, &control);
        }
        if self.client.is_closed() && !self.disconnect_recorded {
            self.disconnect_recorded = true;
            return self.monitor.checkpoint(
                Observation {
                    call_id: format!("transport-closed-{}", self.monitor.sequence + 1),
                    tool: "transport".into(),
                    output: String::new(),
                    error: Some("SDK transport closed".into()),
                },
                Some(Assessment::unknown("SDK transport closed")),
                judge,
                &control,
            );
        }
        Ok(None)
    }
}
