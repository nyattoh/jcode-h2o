//! Pure state for the native reception UI: in-memory sessions, real-chat state driven by
//! [`ChatEvent`]s, explicitly chosen deterministic demo replies, the composer's Enter/IME
//! decision, layout breakpoints and a safe read-only snapshot presentation.
//!
//! Boundaries: nothing here performs I/O to a model, Jev, a daemon or the network (only the
//! controller in [`crate::chat_transport`] does, after an explicit connect), and nothing is
//! persisted. The only I/O is an explicit user-chosen snapshot file, read and validated by the
//! accepted gate in [`crate::ui`]; the view is then decoded from the already-validated bytes with
//! the real monitor types and keeps no observation output/error or context bytes.
use crate::chat_transport::ChatEvent;
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

/// What a conversation talks to, fixed when it is created. Real is the default; Demo is only
/// ever chosen explicitly and never reaches the SDK.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Real,
    Demo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Demo,
    DemoError,
    /// Final assistant text exactly as the SDK reported it.
    Assistant,
    /// A real failure notice. Never a reply and never demo text.
    Notice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

/// The SDK session behind one real conversation. A conversation binds at most once, so a reply
/// can only land in the conversation that asked for it, and old history is never continued by a
/// session that did not see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// Never bound; binds on the next explicit connect or new chat while connected.
    Unbound,
    /// connect/new_session requested; waiting for `Ready`.
    Binding,
    /// Bridge, isolated session and tool exclusion are ready. Says nothing about the provider.
    Ready {
        session_id: String,
        provider: Option<String>,
        model: Option<String>,
    },
    /// A send's outcome is unknown. No further send here: no blind resend.
    Uncertain,
    /// The SDK session is gone or never came up. History stays readable.
    Closed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub request_id: u64,
    pub text: String,
    pub cancelling: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bridge {
    Disconnected { reason: Option<String> },
    Connecting,
    Connected,
}

#[derive(Debug)]
pub struct Session {
    pub id: u64,
    pub title: String,
    pub messages: Vec<Message>,
    pub kind: Kind,
    pub link: Link,
    /// The real request in flight.
    pub request: Option<Pending>,
    /// An actual SDK reply has arrived in this conversation.
    pub verified: bool,
    /// Text of a definitely failed send, handed back to the composer once.
    draft: Option<String>,
    /// (due time, user input) of the pending demo reply.
    pending: Option<(f64, String)>,
}

impl Session {
    pub fn is_loading(&self) -> bool {
        self.pending.is_some() || self.request.is_some()
    }

    pub fn can_send(&self) -> bool {
        match self.kind {
            Kind::Demo => self.pending.is_none(),
            Kind::Real => matches!(self.link, Link::Ready { .. }) && self.request.is_none(),
        }
    }

    fn notice(&mut self, text: String) {
        self.messages.push(Message {
            role: Role::Notice,
            text,
        });
    }
}

const SWITCHED: &str = "別のチャットで新しいセッションを開いたため終了しました";

/// History lives only in this value: it is never written anywhere.
#[derive(Debug)]
pub struct Chat {
    sessions: Vec<Session>,
    active: Option<u64>,
    next_id: u64,
    next_request: u64,
    mode: Kind,
    bridge: Bridge,
    /// Last connection/setup error, shown until the next connect attempt.
    pub error: Option<String>,
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            active: None,
            next_id: 0,
            next_request: 0,
            mode: Kind::Real,
            bridge: Bridge::Disconnected { reason: None },
            error: None,
        }
    }
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
        self.session_mut(id)
    }

    fn session_mut(&mut self, id: u64) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn mode(&self) -> Kind {
        self.mode
    }

    pub fn bridge(&self) -> &Bridge {
        &self.bridge
    }

    /// Explicit mode choice. An active conversation of the other kind is left, not changed.
    pub fn set_mode(&mut self, kind: Kind) {
        self.mode = kind;
        if self.active().is_some_and(|s| s.kind != kind) {
            self.active = None;
        }
    }

    /// Starts a new chat of the current mode, reusing the active one if it is still empty and
    /// usable.
    pub fn new_session(&mut self) -> u64 {
        let mode = self.mode;
        if let Some(s) = self.active().filter(|s| {
            s.messages.is_empty()
                && s.kind == mode
                && matches!(s.link, Link::Unbound | Link::Binding | Link::Ready { .. })
        }) {
            return s.id;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.sessions.push(Session {
            id,
            title: "新しいチャット".into(),
            messages: Vec::new(),
            kind: mode,
            link: Link::Unbound,
            request: None,
            verified: false,
            draft: None,
            pending: None,
        });
        self.active = Some(id);
        id
    }

    pub fn select(&mut self, id: u64) -> bool {
        let Some(kind) = self.sessions.iter().find(|s| s.id == id).map(|s| s.kind) else {
            return false;
        };
        self.active = Some(id);
        self.mode = kind;
        true
    }

    /// Queues a deterministic demo reply in a demo conversation. Refuses blank input, a pending
    /// reply, or a real conversation (which never gets a demo reply).
    pub fn send(&mut self, text: &str, now: f64) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        if self.active.is_none() && self.mode == Kind::Demo {
            self.new_session();
        }
        let Some(s) = self.active_mut() else {
            return false;
        };
        if s.kind != Kind::Demo || !s.can_send() {
            return false;
        }
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

    /// Resolves every due demo reply, in any session. Returns whether anything changed.
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

    // ---- real chat ---------------------------------------------------------------------------

    /// True while a controller event is expected, so the UI keeps polling.
    pub fn awaiting(&self) -> bool {
        self.bridge == Bridge::Connecting
            || self
                .sessions
                .iter()
                .any(|s| s.request.is_some() || s.link == Link::Binding)
    }

    /// The conversation an explicit connect binds: the active one if it is real, empty and never
    /// bound, otherwise a fresh one. Marks it binding.
    pub fn connect_target(&mut self) -> u64 {
        self.mode = Kind::Real;
        let reusable = self.active().is_some_and(|s| {
            s.kind == Kind::Real && s.messages.is_empty() && s.link == Link::Unbound
        });
        let id = if reusable {
            self.active.expect("checked")
        } else {
            self.active = None;
            self.new_session()
        };
        self.error = None;
        self.bridge = Bridge::Connecting;
        self.mark_binding(id);
        id
    }

    /// `id` now owns the controller's single session; any other was closed by the controller.
    pub fn mark_binding(&mut self, id: u64) {
        for s in &mut self.sessions {
            if s.id == id {
                s.link = Link::Binding;
            } else if matches!(s.link, Link::Binding | Link::Ready { .. }) {
                s.link = Link::Closed(SWITCHED.into());
            }
        }
    }

    /// The controller refused locally; nothing was started.
    pub fn rejected(&mut self, id: u64, message: &str) {
        if let Some(s) = self.session_mut(id).filter(|s| s.link == Link::Binding) {
            s.link = Link::Unbound;
        }
        if self.bridge == Bridge::Connecting {
            self.bridge = Bridge::Disconnected { reason: None };
        }
        self.error = Some(one_line(message, 300));
    }

    /// Validates a real send from the active conversation and allocates its request id.
    pub fn prepare(&mut self, text: &str) -> Option<(u64, u64)> {
        let connected = self.bridge == Bridge::Connected;
        let s = self.active()?;
        if text.trim().is_empty() || s.kind != Kind::Real || !s.can_send() || !connected {
            return None;
        }
        let id = s.id;
        self.next_request += 1;
        Some((id, self.next_request))
    }

    /// The controller accepted (enqueued) the send.
    pub fn sent(&mut self, id: u64, request_id: u64, text: &str) {
        let Some(s) = self.session_mut(id) else {
            return;
        };
        if s.messages.is_empty() {
            s.title = one_line(text.trim(), TITLE_CHARS);
        }
        s.messages.push(Message {
            role: Role::User,
            text: text.to_owned(),
        });
        s.request = Some(Pending {
            request_id,
            text: text.to_owned(),
            cancelling: false,
        });
    }

    /// Marks the active request as being cancelled. Returns whether there was one to cancel.
    pub fn cancelling(&mut self) -> bool {
        match self.active_mut().and_then(|s| s.request.as_mut()) {
            Some(p) if !p.cancelling => {
                p.cancelling = true;
                true
            }
            _ => false,
        }
    }

    /// Hands back the draft of a definitely failed send in `id`, once.
    pub fn take_draft(&mut self, id: u64) -> Option<String> {
        self.session_mut(id)?.draft.take()
    }

    /// Applies one controller event. Events for an unknown, other or finished request are
    /// dropped, so a reply is shown once and only in its origin conversation.
    pub fn apply(&mut self, event: ChatEvent) {
        match event {
            ChatEvent::Connecting => {
                self.bridge = Bridge::Connecting;
                self.error = None;
            }
            ChatEvent::Ready {
                conversation_id,
                session_id,
                provider,
                model,
            } => {
                if let Some(s) = self
                    .session_mut(conversation_id)
                    .filter(|s| s.link == Link::Binding)
                {
                    s.link = Link::Ready {
                        session_id,
                        provider,
                        model,
                    };
                    self.bridge = Bridge::Connected;
                }
            }
            ChatEvent::Reply {
                conversation_id,
                request_id,
                text,
            } => {
                if let Some(s) = self.session_mut(conversation_id).filter(|s| {
                    s.request
                        .as_ref()
                        .is_some_and(|p| p.request_id == request_id)
                }) {
                    s.request = None;
                    s.verified = true;
                    s.messages.push(Message {
                        role: Role::Assistant,
                        text,
                    });
                }
            }
            ChatEvent::Failed {
                conversation_id,
                request_id,
                message,
                uncertain,
            } => {
                let message = one_line(&message, 300);
                match (
                    conversation_id.and_then(|c| self.session_mut(c)),
                    request_id,
                ) {
                    (Some(s), Some(r)) => {
                        if !s.request.as_ref().is_some_and(|p| p.request_id == r) {
                            return; // stale: another or an already finished request
                        }
                        let p = s.request.take().expect("checked");
                        if uncertain {
                            s.link = Link::Uncertain;
                            s.notice(format!(
                                "結果不明: {message} — 送信が届いたか確認できません。自動再送しません。新しいチャットで続けてください。"
                            ));
                        } else {
                            s.notice(format!(
                                "送信できませんでした: {message}（入力を下書きに戻しました。再送は送信で明示的に行えます）"
                            ));
                            s.draft = Some(p.text);
                        }
                    }
                    (Some(s), None) => {
                        if matches!(s.link, Link::Binding | Link::Ready { .. }) {
                            s.link = Link::Closed(message.clone());
                        }
                        self.error = Some(message);
                    }
                    (None, _) => self.error = Some(message),
                }
            }
            ChatEvent::Disconnected { message } => {
                let message = one_line(&message, 300);
                for s in &mut self.sessions {
                    if s.request.take().is_some() {
                        // the controller reports this itself first; never leave it pending
                        s.link = Link::Uncertain;
                        s.notice("結果不明: 応答前に切断されました。自動再送しません。".into());
                    } else if matches!(s.link, Link::Binding | Link::Ready { .. }) {
                        s.link = Link::Closed(message.clone());
                    }
                }
                self.bridge = Bridge::Disconnected {
                    reason: Some(message),
                };
            }
        }
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
        // The message embeds the serialized observation, which this view must not keep.
        Action::Replan { .. } => "再計画".into(),
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
