//! Pure state for the native reception UI: in-memory sessions, deterministic demo replies, the
//! composer's Enter/IME decision, layout breakpoints and a safe read-only snapshot presentation.
//!
//! Boundaries: nothing here connects to a model, Jev, a daemon or the network, and nothing is
//! persisted. The only I/O is an explicit user-chosen snapshot file, read and validated by the
//! accepted gate in [`crate::ui`]; the view is then decoded from the already-validated bytes with
//! the real monitor types and keeps no observation output/error or context bytes.
use crate::drift::{Decision, Signal};
use crate::monitor::{Action, BranchSpec, Checkpoint, Receipt};
use crate::ui::{self, SnapshotError};
use serde::Deserialize;
use std::path::Path;

/// Simulated latency of a demo reply, so the loading state is visible.
pub const DEMO_DELAY_SECS: f64 = 0.6;
/// Input that produces the labelled demo error bubble (for checking the error state).
pub const DEMO_ERROR_COMMAND: &str = "/demo-error";
const TITLE_CHARS: usize = 28;

// ---- chat ------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Demo,
    DemoError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

#[derive(Debug)]
pub struct Session {
    pub id: u64,
    pub title: String,
    pub messages: Vec<Message>,
    /// (due time, user input) of the pending demo reply.
    pending: Option<(f64, String)>,
}

impl Session {
    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }
}

/// History lives only in this value: it is never written anywhere.
#[derive(Debug, Default)]
pub struct Chat {
    sessions: Vec<Session>,
    active: Option<u64>,
    next_id: u64,
}

impl Chat {
    /// Oldest first.
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn active_id(&self) -> Option<u64> {
        self.active
    }

    pub fn active(&self) -> Option<&Session> {
        self.sessions.iter().find(|s| Some(s.id) == self.active)
    }

    fn active_mut(&mut self) -> Option<&mut Session> {
        let id = self.active?;
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// Starts a new chat, reusing the active one if it is still empty.
    pub fn new_session(&mut self) -> u64 {
        if let Some(s) = self.active().filter(|s| s.messages.is_empty()) {
            return s.id;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.sessions.push(Session {
            id,
            title: "新しいチャット".into(),
            messages: Vec::new(),
            pending: None,
        });
        self.active = Some(id);
        id
    }

    pub fn select(&mut self, id: u64) -> bool {
        let found = self.sessions.iter().any(|s| s.id == id);
        if found {
            self.active = Some(id);
        }
        found
    }

    /// Queues a deterministic demo reply. Refuses blank input or while a reply is pending.
    pub fn send(&mut self, text: &str, now: f64) -> bool {
        if text.trim().is_empty() || self.active().is_some_and(Session::is_loading) {
            return false;
        }
        if self.active.is_none() {
            self.new_session();
        }
        let s = self.active_mut().expect("active session exists");
        if s.messages.is_empty() {
            s.title = one_line(text.trim(), TITLE_CHARS);
        }
        s.messages.push(Message {
            role: Role::User,
            text: text.to_owned(),
        });
        s.pending = Some((now + DEMO_DELAY_SECS, text.to_owned()));
        true
    }

    /// Resolves every due reply, in any session. Returns whether anything changed.
    pub fn tick(&mut self, now: f64) -> bool {
        let mut changed = false;
        for s in &mut self.sessions {
            if s.pending.as_ref().is_some_and(|(due, _)| *due <= now) {
                let (_, input) = s.pending.take().expect("checked");
                s.messages.push(demo_reply(&input));
                changed = true;
            }
        }
        changed
    }

    pub fn next_due(&self) -> Option<f64> {
        self.sessions
            .iter()
            .filter_map(|s| s.pending.as_ref().map(|p| p.0))
            .reduce(f64::min)
    }
}

/// A fixed, locally generated reply. It never represents a model or Jev answer.
pub fn demo_reply(input: &str) -> Message {
    if input.trim() == DEMO_ERROR_COMMAND {
        return Message {
            role: Role::DemoError,
            text: "【デモのエラー表示】これは表示確認用の擬似エラーです。実際の接続エラーではありません。"
                .into(),
        };
    }
    let chars = input.chars().count();
    let lines = input.lines().count().max(1);
    Message {
        role: Role::Demo,
        text: format!(
            "【デモ応答・未接続】「{}」を受け取りました（{chars}文字・{lines}行）。\n\
             実際のモデル・Jev・daemonには送信していません。この応答は入力から決まる固定のデモです。\n\
             作業対象が未確認のため、ファイルの編集や実行は行いません。",
            one_line(input.trim(), 40)
        ),
    }
}

// ---- composer --------------------------------------------------------------------------------

/// One composer-relevant input event of a frame, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyInput {
    /// Enter pressed; `shift` means Shift+Enter (newline). Other modified Enters are not mapped.
    Enter {
        shift: bool,
    },
    Preedit(String),
    Commit(String),
}

/// Decides whether Enter sends. An Enter during an IME composition, or in a frame with any
/// composition event (before or after it, e.g. the commit Enter), never sends.
#[derive(Debug, Default)]
pub struct ImeGuard {
    composing: bool,
}

impl ImeGuard {
    pub fn is_composing(&self) -> bool {
        self.composing
    }

    pub fn frame(&mut self, events: &[KeyInput]) -> bool {
        let mut ime_seen = false;
        let mut enter = false;
        for event in events {
            match event {
                KeyInput::Preedit(text) => {
                    self.composing = !text.is_empty();
                    ime_seen = true;
                }
                KeyInput::Commit(_) => {
                    self.composing = false;
                    ime_seen = true;
                }
                KeyInput::Enter { shift: false } if !self.composing => enter = true,
                KeyInput::Enter { .. } => {}
            }
        }
        enter && !ime_seen
    }
}

// ---- layout / text ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Sidebar, chat and monitor side by side.
    Wide,
    /// Sidebar and chat; monitor toggled from the header.
    Medium,
    /// One pane at a time, switched by tabs.
    Narrow,
}

pub fn layout_for(width: f32) -> Layout {
    if width < 760.0 {
        Layout::Narrow
    } else if width < 1180.0 {
        Layout::Medium
    } else {
        Layout::Wide
    }
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// One logical row for compact UI: whitespace controls become spaces, other controls and bidi
/// overrides become U+FFFD, and the result is cut to `max` chars with an ellipsis. Width-based
/// truncation to the available cells is done by the widget.
pub fn one_line(s: &str, max: usize) -> String {
    let mut out: String = s
        .chars()
        .take(max)
        .map(|c| match c {
            '\n' | '\t' | '\r' => ' ',
            c if c.is_control() || is_bidi_control(c) => '\u{FFFD}',
            c => c,
        })
        .collect();
    if s.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

/// Literal prose for wrapping labels: keeps newlines, neutralises other controls and bidi.
pub fn prose(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' => '\n',
            '\t' => ' ',
            c if c.is_control() || is_bidi_control(c) => '\u{FFFD}',
            c => c,
        })
        .collect()
}

// ---- snapshot --------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    Yes,
    No,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptState {
    Acknowledged,
    NotAcknowledged,
    /// `receipt: null` in the snapshot: unknown.
    Null,
    /// No receipt entry for this sequence.
    Missing,
}

/// What a checkpoint may show. Observation output/error and context bytes are not kept.
#[derive(Debug)]
pub struct CheckpointRow {
    pub sequence: u64,
    pub call_id: String,
    pub tool: String,
    pub signal: &'static str,
    pub reason: String,
    pub decision: &'static str,
    pub action: String,
    pub receipt: ReceiptState,
}

#[derive(Debug)]
pub struct BranchRow {
    pub run_id: String,
    pub branch_id: String,
    pub session_id: String,
    pub owner_session_id: String,
    pub purpose: String,
    pub constraints: Vec<String>,
    pub success_conditions: Vec<String>,
    pub owned_paths: Vec<String>,
    pub unfinished: Vec<String>,
    pub context_ids: Vec<String>,
    pub max_corrections: u32,
    pub needs_reconciliation: Tri,
    pub checkpoints: Vec<CheckpointRow>,
}

#[derive(Debug)]
pub struct SnapshotView {
    /// Where it came from (file name or "貼り付け"); shown with the read-only label.
    pub source: String,
    pub captured_at: Option<String>,
    pub branches: Vec<BranchRow>,
}

// Decoding shape for bytes that `ui::validate_snapshot` already accepted. Not a second gate.
#[derive(Deserialize)]
struct Doc {
    captured_at: Option<String>,
    branches: Vec<DocBranch>,
}

#[derive(Deserialize)]
struct DocBranch {
    spec: BranchSpec,
    checkpoints: Vec<Checkpoint>,
    receipts: Vec<DocReceipt>,
    needs_reconciliation: Option<bool>,
}

#[derive(Deserialize)]
struct DocReceipt {
    sequence: u64,
    receipt: Option<Receipt>,
}

fn signal_label(s: &Signal) -> &'static str {
    match s {
        Signal::Aligned => "整合",
        Signal::Misaligned => "逸脱",
        Signal::Unknown => "不明",
    }
}

fn decision_label(d: &Decision) -> &'static str {
    match d {
        Decision::Continue => "継続",
        Decision::Correct => "修正",
        Decision::StopBranch => "停止",
        Decision::Hold => "保留",
    }
}

fn action_label(a: &Action) -> String {
    match a {
        Action::Continue => "継続".into(),
        Action::Replan { message } => format!("再計画: {message}"),
        Action::Hold => "保留".into(),
        Action::StopBranch => "停止".into(),
    }
}

fn branch_row(b: DocBranch) -> BranchRow {
    let receipt_of = |seq: u64| {
        // last entry wins if a sequence is listed twice (the gate allows it)
        match b.receipts.iter().rev().find(|r| r.sequence == seq) {
            None => ReceiptState::Missing,
            Some(DocReceipt { receipt: None, .. }) => ReceiptState::Null,
            Some(DocReceipt {
                receipt: Some(r), ..
            }) if r.acknowledged => ReceiptState::Acknowledged,
            Some(_) => ReceiptState::NotAcknowledged,
        }
    };
    let checkpoints = b
        .checkpoints
        .iter()
        .map(|c| CheckpointRow {
            sequence: c.sequence,
            call_id: c.input.observation.call_id.clone(),
            tool: c.input.observation.tool.clone(),
            signal: signal_label(&c.assessment.signal),
            reason: c.assessment.reason.clone(),
            decision: decision_label(&c.decision),
            action: action_label(&c.action),
            receipt: receipt_of(c.sequence),
        })
        .collect();
    let spec = b.spec;
    BranchRow {
        run_id: spec.run_id,
        branch_id: spec.branch_id,
        session_id: spec.session_id,
        owner_session_id: spec.owner_session_id,
        purpose: spec.goal.purpose,
        constraints: spec.goal.constraints,
        success_conditions: spec.goal.success_conditions,
        owned_paths: spec.goal.owned_paths,
        unfinished: spec.goal.unfinished,
        context_ids: spec.context_ids,
        max_corrections: spec.max_corrections,
        needs_reconciliation: match b.needs_reconciliation {
            Some(true) => Tri::Yes,
            Some(false) => Tri::No,
            None => Tri::Unknown,
        },
        checkpoints,
    }
}

/// Validates with the accepted gate, then builds the safe view.
pub fn snapshot_view(bytes: &[u8], source: &str) -> Result<SnapshotView, String> {
    ui::validate_snapshot(bytes).map_err(|e| safe_error(&e))?;
    let doc: Doc =
        serde_json::from_slice(bytes).map_err(|_| "検証後の読み込みに失敗しました".to_owned())?;
    Ok(SnapshotView {
        source: source.to_owned(),
        captured_at: doc.captured_at,
        branches: doc.branches.into_iter().map(branch_row).collect(),
    })
}

/// Gate messages that are fixed text (plus indexes/sequences/static field names) are shown as
/// is. serde messages can quote snapshot keys or values (`unknown field`, `unknown variant`,
/// `invalid type: string "..."`), so they become fixed text plus, at most, the parser position.
fn safe_error(e: &SnapshotError) -> String {
    const FIXED: [&str; 6] = [
        "snapshot must be",
        "captured_at must",
        "branch ",
        "unsupported schema",
        "more than",
        "missing field", // serde names a field of our own struct, never an input key
    ];
    match e {
        SnapshotError::Invalid(m) if FIXED.iter().any(|p| m.starts_with(p)) => {
            format!("形式エラー: {}", one_line(m, 200))
        }
        SnapshotError::Invalid(m) if m.starts_with("unknown field") => {
            "形式エラー: 想定外のフィールドがあります（名前と値は表示しません）".into()
        }
        SnapshotError::Invalid(m) => {
            let at = parser_position(m)
                .map_or(String::new(), |(l, c)| format!(" at line {l} column {c}"));
            format!("JSONまたは型の検証に失敗しました（値は表示しません）{at}")
        }
        SnapshotError::TooLarge => "1 MiBを超えています".into(),
        SnapshotError::NotUtf8 => "UTF-8ではありません".into(),
        SnapshotError::Io(m) => format!("ファイルを読めません: {}", one_line(m, 200)),
    }
}

/// serde_json's own ` at line L column C` suffix, only when it ends the message. A quoted input
/// value can contain that text, but serde always follows a quoted value with trusted text
/// (`, expected ...`), so an input-controlled match cannot reach the end. Only the numbers leave.
fn parser_position(m: &str) -> Option<(u64, u64)> {
    let (head, column) = m.rsplit_once(" column ")?;
    let (_, line) = head.rsplit_once(" at line ")?;
    let digits = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| s.parse().ok())
            .flatten()
    };
    Some((digits(line)?, digits(column)?))
}

/// Imported, read-only monitoring data. An invalid import keeps the previous snapshot.
#[derive(Debug, Default)]
pub struct Monitor {
    pub snapshot: Option<SnapshotView>,
    pub last_error: Option<String>,
}

impl Monitor {
    fn apply(&mut self, result: Result<SnapshotView, String>) -> bool {
        match result {
            Ok(view) => {
                self.snapshot = Some(view);
                self.last_error = None;
                true
            }
            Err(e) => {
                self.last_error = Some(e);
                false
            }
        }
    }

    pub fn import_pasted(&mut self, text: &str) -> bool {
        self.apply(snapshot_view(text.as_bytes(), "貼り付け"))
    }

    pub fn import_file(&mut self, path: &Path) -> bool {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let result = ui::load_snapshot(path)
            .map_err(|e| safe_error(&e))
            .and_then(|b| snapshot_view(&b, &name));
        self.apply(result)
    }
}
