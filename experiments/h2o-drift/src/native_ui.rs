//! egui view of the native reception UI. State and decisions live in [`crate::native`]; this file
//! lays out widgets, maps egui input to [`KeyInput`], loads a system Japanese font and forwards
//! explicit user actions (接続 / 送信 / キャンセル / 切断 / 新しいチャット) to the
//! [`ChatController`], whose events it drains each frame without blocking. It connects only when
//! the user clicks 接続 and never writes files.
use crate::chat_transport::{ChatController, ConnectConfig};
use crate::native::{
    BranchRow, Bridge, Chat, ImeGuard, KeyInput, Kind, Layout, Link, Monitor, ReceiptState, Role,
    Session, Tri, display_path, layout_for, one_line, prose,
};
use crate::work_target::{
    Permission, WorkSessionPlan, WorkTarget, WorkTargetError, WorkTargetState,
};
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Key,
    KeyboardShortcut, Label, Margin, Modifiers, RichText, ScrollArea, Stroke, TextEdit, TextStyle,
    Theme,
};
use std::{path::Path, sync::Arc};

// ---- font ------------------------------------------------------------------------------------

/// Known system Japanese fonts (face 0 of each collection). Nothing is bundled or downloaded.
const FONT_CANDIDATES: [&str; 7] = [
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontStatus {
    Loaded { path: String },
    Missing,
}

/// Loads the first readable candidate as the primary font (Latin included, so mixed text shares
/// one baseline) and as a monospace fallback. Japanese system fonts draw `\` as a yen sign; the
/// character itself is unchanged. Without a font, Japanese renders as boxes and the UI says so.
pub fn install_japanese_font(ctx: &egui::Context) -> FontStatus {
    let Some((path, bytes)) = FONT_CANDIDATES
        .iter()
        .find_map(|p| std::fs::read(p).ok().map(|b| (*p, b)))
    else {
        return FontStatus::Missing;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("jp".into(), Arc::new(FontData::from_owned(bytes)));
    if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
        f.insert(0, "jp".into());
    }
    if let Some(f) = fonts.families.get_mut(&FontFamily::Monospace) {
        f.push("jp".into());
    }
    ctx.set_fonts(fonts);
    FontStatus::Loaded { path: path.into() }
}

// ---- palette ---------------------------------------------------------------------------------

/// Every explicit paint in this file comes from the palette of the [`egui::Ui`] it paints into, so
/// a theme switch recolours all panels, bubbles, chips, banners and errors at once.
struct Palette {
    bg: Color32,
    sidebar: Color32,
    surface: Color32,
    hover: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    selection: Color32,
    warn: Color32,
    warn_bg: Color32,
    error: Color32,
    error_bg: Color32,
    ok: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const LIGHT: Palette = Palette {
    bg: rgb(0xf7f8fa),
    sidebar: rgb(0xeef0f3),
    surface: rgb(0xffffff),
    hover: rgb(0xe3e7ed),
    border: rgb(0xc9cfd8),
    text: rgb(0x1f2328),
    muted: rgb(0x57606a),
    accent: rgb(0x0b57d0),
    selection: rgb(0xd3e3fd),
    warn: rgb(0x7a4f00),
    warn_bg: rgb(0xfff4d6),
    error: rgb(0xb3261e),
    error_bg: rgb(0xfdeceb),
    ok: rgb(0x1e6b35),
};

const DARK: Palette = Palette {
    bg: rgb(0x0e1014),
    sidebar: rgb(0x15171c),
    surface: rgb(0x1c1f26),
    hover: rgb(0x262a33),
    border: rgb(0x2a2e37),
    text: rgb(0xe6e8ec),
    muted: rgb(0x9aa0ab),
    accent: rgb(0x8ab4f8),
    selection: rgb(0x2b3d5c),
    warn: rgb(0xf2b84b),
    warn_bg: rgb(0x2a2210),
    error: rgb(0xf28b82),
    error_bg: rgb(0x2c1616),
    ok: rgb(0x81c995),
};

fn palette(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Light => &LIGHT,
        Theme::Dark => &DARK,
    }
}

fn pal(ui: &egui::Ui) -> &'static Palette {
    palette(if ui.visuals().dark_mode {
        Theme::Dark
    } else {
        Theme::Light
    })
}

fn style_for(theme: Theme) -> egui::Style {
    let p = palette(theme);
    let mut style = egui::Style::default();
    let mut v = theme.default_visuals();
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.sidebar;
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.bg_fill = p.bg;
    v.widgets.noninteractive.weak_bg_fill = p.bg;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.bg_fill = p.surface;
    v.widgets.inactive.weak_bg_fill = p.surface;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.active.bg_fill = p.selection;
    v.widgets.active.weak_bg_fill = p.selection;
    v.widgets.open.bg_fill = p.hover;
    v.widgets.open.weak_bg_fill = p.hover;
    style.visuals = v;
    for (ts, size) in [
        (TextStyle::Heading, 22.0),
        (TextStyle::Body, 15.0),
        (TextStyle::Button, 15.0),
        (TextStyle::Small, 12.0),
        (TextStyle::Monospace, 13.0),
    ] {
        let family = if ts == TextStyle::Monospace {
            FontFamily::Monospace
        } else {
            FontFamily::Proportional
        };
        style.text_styles.insert(ts, FontId::new(size, family));
    }
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style
}

/// Installs both styles and selects `theme` explicitly, so the OS theme never overrides it.
fn apply_theme(ctx: &egui::Context, theme: Theme) {
    for t in [Theme::Light, Theme::Dark] {
        ctx.set_style_of(t, style_for(t));
    }
    ctx.set_theme(theme);
}

// ---- provenance ------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Prov {
    Fact,
    Demo,
    Unknown,
    Imported,
    /// What would be done; nothing is running.
    Plan,
}

impl Prov {
    fn chip(self, p: &Palette) -> (&'static str, Color32) {
        match self {
            Prov::Fact => ("事実", p.ok),
            Prov::Demo => ("デモ", p.warn),
            Prov::Unknown => ("不明", p.muted),
            Prov::Imported => ("取込", p.accent),
            Prov::Plan => ("計画", p.muted),
        }
    }
}

/// The validated work-target state as one status line.
fn work_target_status(t: &WorkTarget) -> String {
    match (t.confirmed_dir(), t.permission()) {
        (Some(dir), Some(p)) => {
            format!("作業対象: 確定済み: {} · {}", display_path(dir), p.label())
        }
        _ => format!("作業対象: {}", t.state().label()),
    }
}

/// The chat's own restriction, which a confirmed work target does not lift.
const CHAT_RESTRICTION: &str =
    "チャット: ツール無効・自動承認なし（作業対象を確定しても編集・実行しません）";

/// One compact status row: provenance chip + a single truncated line.
fn row(ui: &mut egui::Ui, prov: Prov, text: &str) {
    ui.horizontal(|ui| {
        let (label, color) = prov.chip(pal(ui));
        Frame::new()
            .stroke(Stroke::new(1.0, color))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(4, 0))
            .show(ui, |ui| ui.label(RichText::new(label).small().color(color)));
        ui.add(Label::new(one_line(text, 300)).truncate());
    });
}

/// This app writes no history file; the jcode daemon/runtime side was not observed and may keep
/// what a real chat sent (current runtime source persists session metadata after a prompt).
const HISTORY_SCOPE: &str = "この画面の履歴はメモリ内のみ（このアプリはファイルに保存しません）。ただし実会話で送った内容は、jcode側（daemon/runtime）に保存される場合があります。アプリを閉じても削除されたとは限りません。デモの内容はどこにも送りません。";

/// Shown in real mode before any prompt: storage scope plus the C1 per-turn session and the
/// replay rule of chat_transport::replay: the most recent contiguous accepted whole turns whose
/// q+a body chars total ≤ MAX_REPLAY_CHARS (32 000), rendered chronologically; labels/separators/
/// omission note come on top, and an oversize newest turn drops all earlier context.
const REAL_CHAT_NOTICE: &str = "保存と文脈: この画面の履歴はメモリ内のみですが、送信内容はjcode側（daemon/runtime）に保存される場合があります。アプリを閉じても削除されたとは限りません。送信ごとに新しい隔離セッションを使います。2回目以降は、このチャットで応答済みのやり取りを往復単位で、直近から連続する分だけ質問と応答の本文合計32,000文字以内で選び、時系列順に添えて送ります（見出し・区切り・省略の説明は別に加わります。収まらない古い分は省略し、直近の1往復だけで超える場合はそれ以前の文脈を全て省略。失敗・キャンセルの分は送りません。この32,000文字は今回の入力・送信全体・画面の履歴・トークン数の上限ではありません。文脈の引き継ぎは対応するjcodeが前提）。スナップショットは送りません。";

fn line(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) {
    ui.add(Label::new(text).truncate());
}

fn tri(t: Tri) -> &'static str {
    match t {
        Tri::Yes => "要",
        Tri::No => "不要",
        Tri::Unknown => "不明（null）",
    }
}

fn receipt(r: ReceiptState) -> &'static str {
    match r {
        ReceiptState::Acknowledged => "受領済み",
        ReceiptState::NotAcknowledged => "未受領",
        ReceiptState::Null => "不明（null）",
        ReceiptState::Missing => "記録なし",
    }
}

const SHOWN_CHECKPOINTS: usize = 20;

// ---- app -------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Chat,
    History,
    Monitor,
}

pub struct NativeApp {
    controller: ChatController,
    endpoint_input: String,
    chat: Chat,
    monitor: Monitor,
    composer: String,
    guard: ImeGuard,
    font: FontStatus,
    layout: Layout,
    pane: Pane,
    monitor_open: bool,
    focus_composer: bool,
    theme: Theme,
    applied: Option<Theme>,
    path_input: String,
    paste_input: String,
    notice: Option<String>,
    work_input: String,
    work_target: WorkTarget,
    work_error: Option<WorkTargetError>,
    /// Produced on 確定; a data value only, no work session is started from it.
    work_plan: Option<WorkSessionPlan>,
    #[cfg(feature = "embedded-runtime")]
    embedded_runtime: Option<crate::embedded_runtime::EmbeddedRuntime>,
}

impl eframe::App for NativeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl NativeApp {
    /// Real controller over the platform socket. Nothing connects until the user clicks 接続.
    pub fn new(font: FontStatus) -> Self {
        Self::with_controller(font, ChatController::new())
    }

    pub fn with_controller(font: FontStatus, controller: ChatController) -> Self {
        Self {
            controller,
            endpoint_input: String::new(),
            chat: Chat::default(),
            monitor: Monitor::default(),
            composer: String::new(),
            guard: ImeGuard::default(),
            font,
            layout: Layout::Wide,
            pane: Pane::Chat,
            monitor_open: false,
            focus_composer: true,
            theme: Theme::Light,
            applied: None,
            path_input: String::new(),
            paste_input: String::new(),
            notice: None,
            work_input: String::new(),
            work_target: WorkTarget::default(),
            work_error: None,
            work_plan: None,
            #[cfg(feature = "embedded-runtime")]
            embedded_runtime: None,
        }
    }

    /// Explicit choice between real chat and the local demo. Never connects by itself.
    pub fn set_mode(&mut self, kind: Kind) {
        self.chat.set_mode(kind);
        self.focus_composer = true;
    }

    pub fn chat(&self) -> &Chat {
        &self.chat
    }
    pub fn chat_mut(&mut self) -> &mut Chat {
        &mut self.chat
    }
    pub fn composer(&self) -> &str {
        &self.composer
    }
    pub fn layout(&self) -> Layout {
        self.layout
    }
    pub fn work_target(&self) -> &WorkTarget {
        &self.work_target
    }
    pub fn monitor_mut(&mut self) -> &mut Monitor {
        &mut self.monitor
    }

    pub fn show_monitor_pane(&mut self) {
        self.pane = Pane::Monitor;
        self.monitor_open = true;
    }

    /// A new conversation. While connected, a real one gets its own new isolated SDK session
    /// (the controller closes the previous one); old conversations are never re-bound.
    pub fn new_chat(&mut self) {
        if !self.can_start_chat() {
            return;
        }
        let id = self.chat.new_session();
        let unbound = self
            .chat
            .active()
            .is_some_and(|s| s.kind == Kind::Real && s.link == Link::Unbound);
        if unbound && *self.chat.bridge() == Bridge::Connected {
            match self.controller.new_session(id) {
                Ok(()) => self.chat.mark_binding(id),
                Err(e) => self.chat.rejected(id, &e),
            }
        }
        self.pane = Pane::Chat;
        self.focus_composer = true;
    }

    /// The kind of the active conversation, or the chosen mode when none is active.
    fn kind(&self) -> Kind {
        self.chat.active().map_or(self.chat.mode(), |s| s.kind)
    }

    /// A real turn or session setup in flight blocks opening another session.
    fn can_start_chat(&self) -> bool {
        self.chat.mode() == Kind::Demo || !self.chat.awaiting()
    }

    #[cfg(feature = "embedded-runtime")]
    pub fn is_embedded_runtime_running(&self) -> bool {
        self.embedded_runtime.is_some()
    }

    pub fn set_endpoint_input(&mut self, endpoint: impl Into<String>) {
        self.endpoint_input = endpoint.into();
    }

    /// Explicit disconnect: closes client connection and stops any embedded runtime.
    pub fn disconnect(&mut self) {
        self.controller.disconnect();
        #[cfg(feature = "embedded-runtime")]
        {
            self.embedded_runtime = None;
        }
    }

    /// Explicit connect to an existing jcode endpoint or to the in-process embedded runtime.
    pub fn connect(&mut self) {
        let endpoint = self.endpoint_input.trim().trim_matches('"').to_owned();
        #[cfg(feature = "embedded-runtime")]
        let endpoint = if endpoint.is_empty() {
            match crate::embedded_runtime::EmbeddedRuntime::start() {
                Ok(runtime) => {
                    let socket_path = runtime.api_socket().to_string_lossy().into_owned();
                    self.embedded_runtime = Some(runtime);
                    socket_path
                }
                Err(e) => {
                    self.chat.error = Some(format!("内蔵ランタイムの起動に失敗しました: {e}"));
                    return;
                }
            }
        } else {
            self.embedded_runtime = None;
            endpoint
        };
        #[cfg(not(feature = "embedded-runtime"))]
        if endpoint.is_empty() {
            self.chat.error = Some("エンドポイントを入力してください".into());
            return;
        }

        let id = self.chat.connect_target();
        let config = ConnectConfig {
            endpoint: endpoint.into(),
            ..ConnectConfig::default()
        };
        if let Err(e) = self.controller.connect(config, id) {
            #[cfg(feature = "embedded-runtime")]
            {
                self.embedded_runtime = None;
            }
            self.chat.rejected(id, &e);
        }
        self.pane = Pane::Chat;
    }

    fn send(&mut self, now: f64) {
        let text = self.composer.trim().to_owned();
        let accepted = if self.kind() == Kind::Demo {
            self.chat.send(&text, now)
        } else if let Some((id, request)) = self.chat.prepare(&text) {
            match self.controller.send(id, request, text.clone()) {
                Ok(()) => {
                    self.chat.sent(id, request, &text);
                    true
                }
                Err(e) => {
                    self.chat.error = Some(one_line(&e, 300));
                    false
                }
            }
        } else {
            false
        };
        if accepted {
            self.composer.clear();
            self.pane = Pane::Chat;
        }
        self.focus_composer = true;
    }

    /// Drains controller events (never blocks) and keeps polling while one is expected.
    fn poll(&mut self, ctx: &egui::Context) {
        while let Some(event) = self.controller.try_recv() {
            #[cfg(feature = "embedded-runtime")]
            if matches!(event, crate::chat_transport::ChatEvent::Disconnected { .. }) {
                self.embedded_runtime = None;
            }
            self.chat.apply(event);
        }
        if let Some(id) = self.chat.active_id()
            && self.composer.trim().is_empty()
            && let Some(draft) = self.chat.take_draft(id)
        {
            self.composer = draft;
            self.focus_composer = true;
        }
        if self.chat.awaiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    pub fn select_session(&mut self, id: u64) -> bool {
        let ok = self.chat.select(id);
        if ok {
            self.pane = Pane::Chat;
            self.focus_composer = true;
        }
        ok
    }

    /// Explicit, user-chosen file import (GUI path field or `--snapshot`).
    pub fn import_snapshot_file(&mut self, path: &Path) -> bool {
        self.monitor.import_file(path)
    }

    /// One frame. Public so tests can drive real egui passes headlessly.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.applied != Some(self.theme) {
            apply_theme(ui.ctx(), self.theme);
            ui.set_style(ui.ctx().global_style()); // this pass's root Ui predates the switch
            self.applied = Some(self.theme);
        }
        let p = pal(ui);
        let now = ui.input(|i| i.time);
        self.poll(ui.ctx());
        self.chat.tick(now);
        if let Some(due) = self.chat.next_due() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
        self.layout = layout_for(ui.available_width());

        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| self.header(ui));

        match self.layout {
            Layout::Narrow => {
                egui::Panel::top("tabs")
                    .frame(
                        Frame::new()
                            .fill(p.bg)
                            .inner_margin(Margin::symmetric(12, 4)),
                    )
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (pane, label) in [
                                (Pane::Chat, "チャット"),
                                (Pane::History, "履歴"),
                                (Pane::Monitor, "監視"),
                            ] {
                                if ui.selectable_label(self.pane == pane, label).clicked() {
                                    self.pane = pane;
                                }
                            }
                        });
                    });
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(p.bg).inner_margin(Margin::same(10)))
                    .show(ui, |ui| match self.pane {
                        Pane::Chat => self.chat_ui(ui),
                        Pane::History => self.history_ui(ui),
                        Pane::Monitor => self.monitor_ui(ui),
                    });
            }
            layout => {
                egui::Panel::left("sidebar")
                    .resizable(true)
                    .default_size(250.0)
                    .size_range(180.0..=360.0)
                    .frame(Frame::new().fill(p.sidebar).inner_margin(Margin::same(12)))
                    .show(ui, |ui| self.history_ui(ui));
                if layout == Layout::Wide || self.monitor_open {
                    egui::Panel::right("monitor")
                        .resizable(true)
                        .default_size(340.0)
                        .size_range(260.0..=520.0)
                        .frame(Frame::new().fill(p.sidebar).inner_margin(Margin::same(12)))
                        .show(ui, |ui| self.monitor_ui(ui));
                }
                egui::CentralPanel::default()
                    .frame(
                        Frame::new()
                            .fill(p.bg)
                            .inner_margin(Margin::symmetric(16, 10)),
                    )
                    .show(ui, |ui| self.chat_ui(ui));
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            // right-to-left first so the switch keeps its room and the title truncates instead
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (theme, label, hint) in [
                    (Theme::Dark, "ダーク", "ダークモード（暗い表示）に切り替え"),
                    (
                        Theme::Light,
                        "ライト",
                        "ライトモード（明るい表示）に切り替え",
                    ),
                ] {
                    if ui
                        .selectable_value(&mut self.theme, theme, label)
                        .on_hover_text(hint)
                        .changed()
                    {
                        ui.ctx().request_repaint();
                    }
                }
                ui.add(Label::new(RichText::new("表示").small().color(p.muted)));
                if self.layout == Layout::Medium {
                    let label = if self.monitor_open {
                        "監視パネルを閉じる"
                    } else {
                        "監視パネル"
                    };
                    if ui.button(label).clicked() {
                        self.monitor_open = !self.monitor_open;
                    }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(Label::new(RichText::new("H₂O 受付").strong().size(17.0)).truncate());
                });
            });
        });
        Frame::new()
            .fill(p.warn_bg)
            .stroke(Stroke::new(1.0, p.warn))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // the restriction comes before the target so truncation never hides it
                let status = work_target_status(&self.work_target);
                let (first, second) = if self.kind() == Kind::Demo {
                    (
                        "デモ・未接続 — モデル/Jev/daemonには接続しません。応答は固定のデモです。",
                        format!("チャットは編集・実行しません · {status}"),
                    )
                } else {
                    (
                        "実会話 — 既存のjcodeに明示的に接続します（自動起動しません・ログインや鍵の読み取りもしません）",
                        format!("ツール無効化を確認してから送信 · チャットは編集・実行しません · {status} · H₂O/Jev/Swarm/メモリは不明のまま"),
                    )
                };
                line(ui, RichText::new(first).color(p.warn));
                line(ui, RichText::new(second).color(p.warn).small());
            });
    }

    fn history_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let full = ui.available_width();
        let enabled = self.can_start_chat();
        if ui
            .add_enabled_ui(enabled, |ui| {
                ui.add_sized(
                    [full, 34.0],
                    egui::Button::new("＋ 新しいチャット").fill(p.surface),
                )
            })
            .inner
            .on_disabled_hover_text(
                "応答またはセッション準備の完了後に作れます（キャンセルできます）",
            )
            .clicked()
        {
            self.new_chat();
        }
        ui.add_space(6.0);
        line(ui, RichText::new("履歴").color(p.muted).small());
        let mut clicked = None;
        egui::Panel::bottom("history-scope")
            .frame(Frame::new().inner_margin(Margin::symmetric(0, 6)))
            .show(ui, |ui| {
                ui.add(Label::new(RichText::new(HISTORY_SCOPE).color(p.muted).small()).wrap());
            });
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.chat.sessions().is_empty() {
                    line(ui, RichText::new("履歴はまだありません").color(p.muted));
                }
                for s in self.chat.sessions().iter().rev() {
                    let selected = self.chat.active_id() == Some(s.id);
                    let mut title = one_line(&s.title, 60);
                    if s.is_loading() {
                        title.push_str(" …");
                    }
                    let r = ui.add_sized(
                        [ui.available_width(), 30.0],
                        egui::Button::selectable(selected, title).truncate(),
                    );
                    if r.clicked() {
                        clicked = Some(s.id);
                    }
                }
            });
        if let Some(id) = clicked {
            self.select_session(id);
        }
    }

    fn chat_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        // composer first: panels claim space in call order, so 送信/キャンセル keep their room
        // at the supported minimum window (360×480) however long the notice, route or error is
        egui::Panel::bottom("composer")
            .frame(Frame::new().inner_margin(Margin::symmetric(0, 8)))
            .show(ui, |ui| self.composer_ui(ui));
        let room = ui.available_height();
        egui::Panel::top("connection")
            .frame(Frame::new().inner_margin(Margin::symmetric(0, 6)))
            .show(ui, |ui| self.connection_ui(ui, room));
        egui::CentralPanel::default()
            .frame(Frame::new())
            .show(ui, |ui| {
                let empty = self.chat.active().is_none_or(|s| s.messages.is_empty());
                if empty {
                    self.welcome_ui(ui);
                    return;
                }
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let s = self.chat.active().expect("not empty");
                        for m in &s.messages {
                            let (who, fill, color) = match m.role {
                                Role::User => ("あなた", p.surface, p.muted),
                                Role::Demo => ("デモ応答（未接続）", p.warn_bg, p.warn),
                                Role::DemoError => ("デモのエラー（擬似）", p.error_bg, p.error),
                                Role::Assistant => ("AI（jcode・実応答）", p.sidebar, p.accent),
                                Role::Notice => {
                                    ("エラー（実接続・応答ではありません）", p.error_bg, p.error)
                                }
                            };
                            ui.add_space(4.0);
                            line(ui, RichText::new(who).small().color(color));
                            Frame::new()
                                .fill(fill)
                                .stroke(Stroke::new(
                                    1.0,
                                    if m.role == Role::User {
                                        p.border
                                    } else {
                                        color
                                    },
                                ))
                                .corner_radius(CornerRadius::same(12))
                                .inner_margin(Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.set_max_width(ui.available_width());
                                    ui.add(Label::new(prose(&m.text)).wrap());
                                });
                        }
                        if s.is_loading() {
                            let waiting = match &s.request {
                                None => "デモ応答を生成中…",
                                Some(r) if r.cancelling => "キャンセルを確認しています…",
                                Some(_) => "実応答を待っています…",
                            };
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                line(ui, RichText::new(waiting).color(p.muted));
                            });
                        }
                    });
            });
    }

    fn welcome_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.25).min(140.0));
            ui.add(Label::new(RichText::new("何をお手伝いしましょう？").heading()).wrap());
            ui.add_space(6.0);
            if self.kind() == Kind::Real {
                ui.add(
                    Label::new(
                        RichText::new("既存のjcodeに接続すると、送信した文章と、このチャットで応答済みのやり取りだけがjcode経由でAIに送られます。取り込んだスナップショットは送りません。")
                            .color(p.muted),
                    )
                    .wrap(),
                );
                return;
            }
            ui.add(
                Label::new(
                    RichText::new("これはデモです。送信内容はどこにも送られず、決まった形のデモ応答が返ります。")
                        .color(p.muted),
                )
                .wrap(),
            );
            ui.add_space(12.0);
            for s in ["H₂Oの監視状態を見たい", "スナップショットの取り込み方は？", DEMO_ERROR_HINT] {
                if ui.button(one_line(s, 40)).clicked() {
                    self.composer = if s == DEMO_ERROR_HINT { crate::native::DEMO_ERROR_COMMAND.into() } else { s.into() };
                    self.focus_composer = true;
                }
            }
        });
    }

    /// Mode switch, endpoint field, 接続/切断, bridge/session status, reported route and errors.
    /// `room` is the height above the composer; the details scroll within part of it.
    fn connection_ui(&mut self, ui: &mut egui::Ui, room: f32) {
        let p = pal(ui);
        let kind = self.kind();
        ui.horizontal(|ui| {
            for (k, label, hint) in [
                (
                    Kind::Real,
                    "実会話",
                    "既存のjcode経由で実際のAIと会話します",
                ),
                (
                    Kind::Demo,
                    "デモモード",
                    "どこにも送らない固定のデモ応答で表示を確認します",
                ),
            ] {
                if ui
                    .add(egui::Button::selectable(kind == k, label))
                    .on_hover_text(hint)
                    .clicked()
                    && kind != k
                {
                    self.set_mode(k);
                }
            }
            #[cfg(feature = "embedded-runtime")]
            if kind == Kind::Real {
                ui.label(RichText::new("内蔵ランタイム対応").small().color(p.ok));
            }
        });
        if kind == Kind::Demo {
            return;
        }
        let bridge = self.chat.bridge().clone();
        ui.horizontal(|ui| {
            let button_w = 64.0;
            let hint_text = if cfg!(feature = "embedded-runtime") {
                "空欄: 内蔵ランタイムを使用 / または既存jcodeのソケットパス（例: …/jcode-api.sock）"
            } else {
                "既存jcodeのソケットパス（例: …/jcode-api.sock）"
            };
            ui.add_enabled(
                matches!(bridge, Bridge::Disconnected { .. }),
                TextEdit::singleline(&mut self.endpoint_input)
                    .hint_text(hint_text)
                    .desired_width((ui.available_width() - button_w - 8.0).max(40.0)),
            );
            let (label, connect) = match bridge {
                Bridge::Disconnected { .. } => ("接続", true),
                _ => ("切断", false),
            };
            #[cfg(feature = "embedded-runtime")]
            let enabled = true;
            #[cfg(not(feature = "embedded-runtime"))]
            let enabled = !connect || !self.endpoint_input.trim().is_empty();
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(label).min_size(egui::vec2(button_w, 0.0)),
                )
                .clicked()
            {
                if connect {
                    self.connect();
                } else {
                    self.disconnect();
                }
            }
        });
        let active = self.chat.active();
        let ready = active.and_then(|s| match &s.link {
            Link::Ready {
                provider, model, ..
            } => Some((s, provider, model)),
            _ => None,
        });
        let status = match (&bridge, ready) {
            (Bridge::Disconnected { reason: None }, _) => {
                "未接続 — 接続するまで何も送りません".to_owned()
            }
            (Bridge::Disconnected { reason: Some(r) }, _) => format!("未接続 — {r}"),
            (Bridge::Connecting, _) => {
                "接続中…（ブリッジ・隔離セッション・ツール無効化を確認しています）".into()
            }
            // logical readiness: C1 holds no socket between turns and re-checks on every Send
            (Bridge::Connected, Some(_)) => {
                #[cfg(feature = "embedded-runtime")]
                if self.embedded_runtime.is_some() {
                    "接続設定済み（内蔵ランタイム・プロセス内IPC） — 送信ごとに再確認（隔離セッション・ツール無効化）".into()
                } else {
                    "接続設定済み — 送信ごとに再確認（隔離セッション・ツール無効化）".into()
                }
                #[cfg(not(feature = "embedded-runtime"))]
                "接続設定済み — 送信ごとに再確認（隔離セッション・ツール無効化）".into()
            }
            (Bridge::Connected, None) => {
                "このチャットは未準備です（jcodeの接続設定は別のチャットで済み）".into()
            }
        };
        let color = match bridge {
            Bridge::Connected => p.ok,
            Bridge::Connecting => p.muted,
            Bridge::Disconnected { .. } => p.warn,
        };
        line(ui, RichText::new(status).color(color));
        // the rest scrolls in a bounded area (at most half, leaving the messages ≥ 60 px when
        // possible), so a long error/notice/route never pushes the composer away and the notice
        // stays readable by scrolling
        let left = room - ui.min_rect().height();
        let extra = if left > 500.0 { 24.0 } else { 0.0 };
        let max = (left * 0.5 + extra).min(left - 60.0).max(36.0);
        ScrollArea::vertical()
            .id_salt("connection-details")
            .max_height(max)
            .min_scrolled_height(max)
            .auto_shrink([false, true])
            .show(ui, |ui| self.connection_details(ui, ready));
    }

    fn connection_details(
        &self,
        ui: &mut egui::Ui,
        ready: Option<(&Session, &Option<String>, &Option<String>)>,
    ) {
        let p = pal(ui);
        if let Some(err) = &self.chat.error {
            Frame::new()
                .fill(p.error_bg)
                .stroke(Stroke::new(1.0, p.error))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        Label::new(
                            RichText::new(format!("接続エラー: {}", prose(err))).color(p.error),
                        )
                        .wrap(),
                    );
                });
        }
        // essential, so it wraps instead of hiding behind a truncation tooltip
        ui.add(Label::new(RichText::new(REAL_CHAT_NOTICE).small().color(p.muted)).wrap());
        if let Some((s, provider, model)) = ready {
            let route = match (provider, model) {
                (None, None) => "経路（jcodeの報告）: 未報告".to_owned(),
                (p, m) => format!(
                    "経路（jcodeの報告）: {} / {}",
                    p.as_deref().unwrap_or("未報告"),
                    m.as_deref().unwrap_or("未報告")
                ),
            };
            let verified = if s.verified {
                "実応答: 受信済み（このチャット）"
            } else {
                "初回の実応答: 未確認（送信して返答が届くまで分かりません）"
            };
            line(ui, RichText::new(verified).small().color(p.muted));
            line(ui, RichText::new(route).small().color(p.muted));
        }
    }

    fn composer_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        let active = self.chat.active();
        let loading = active.is_some_and(Session::is_loading);
        let request = active.and_then(|s| s.request.as_ref());
        let cancellable = request.is_some_and(|r| !r.cancelling);
        let real_turn = request.is_some();
        let sendable = match (self.kind(), active) {
            (Kind::Demo, s) => s.is_none_or(Session::can_send),
            (Kind::Real, Some(s)) => s.can_send() && *self.chat.bridge() == Bridge::Connected,
            (Kind::Real, None) => false,
        };
        let id = ui.make_persistent_id("composer");
        let was_focused = ui.memory(|m| m.has_focus(id));
        let mut send = false;
        let mut cancel = false;
        Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let button_w = 80.0;
                    let edit = TextEdit::multiline(&mut self.composer)
                        .id(id)
                        .hint_text("メッセージを入力（Enterで送信・Shift+Enterで改行）")
                        .desired_rows(2)
                        .frame(Frame::NONE)
                        .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                        .desired_width(ui.available_width() - button_w - 8.0);
                    let r = ui.add(edit);
                    if self.focus_composer {
                        r.request_focus();
                        self.focus_composer = false;
                    }
                    if real_turn {
                        let b = ui.add_enabled(
                            cancellable,
                            egui::Button::new("キャンセル").min_size(egui::vec2(button_w, 32.0)),
                        );
                        cancel = b.clicked();
                    } else {
                        let can_send = sendable && !self.composer.trim().is_empty();
                        let b = ui.add_enabled(
                            can_send,
                            egui::Button::new("送信").min_size(egui::vec2(button_w, 32.0)),
                        );
                        send = b.clicked();
                    }
                });
            });
        let focused = was_focused || ui.memory(|m| m.has_focus(id));
        if focused {
            let events = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key {
                            key: Key::Enter,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            if modifiers.is_none() {
                                Some(KeyInput::Enter { shift: false })
                            } else if *modifiers == Modifiers::SHIFT {
                                Some(KeyInput::Enter { shift: true })
                            } else {
                                None
                            }
                        }
                        egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                            Some(KeyInput::Preedit(text.clone()))
                        }
                        egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                            Some(KeyInput::Commit(text.clone()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            });
            send |= self.guard.frame(&events);
        } else {
            // a composition cannot outlive focus; egui interrupts it on blur
            self.guard = ImeGuard::default();
        }
        if cancel && self.chat.cancelling() {
            self.controller.cancel();
        }
        if send {
            self.send(ui.input(|i| i.time));
        }
        let hint = self.composer_hint(loading);
        line(ui, RichText::new(hint).small().color(p.muted));
    }

    fn composer_hint(&self, loading: bool) -> &'static str {
        let active = self.chat.active();
        if self.kind() == Kind::Demo {
            return if loading {
                "デモ応答の生成中は送信できません"
            } else {
                "Enterで送信 · Shift+Enterで改行 · 変換中のEnterは送信しません"
            };
        }
        match (self.chat.bridge(), active.map(|s| &s.link)) {
            (_, Some(Link::Uncertain)) => {
                "前回の送信結果が不明です。自動再送しません — 新しいチャットで続けてください"
            }
            (_, Some(Link::Closed(_))) => {
                "このチャットのセッションは終了しました — 新しいチャットで続けてください"
            }
            _ if loading => "実応答を待っています — キャンセルできます",
            (Bridge::Connecting, _) | (_, Some(Link::Binding)) => "セッションを準備中です…",
            (Bridge::Connected, Some(Link::Ready { .. })) => {
                "Enterで送信 · Shift+Enterで改行 · 変換中のEnterは送信しません"
            }
            _ => {
                "未接続です — 上でエンドポイントを指定して接続してください（下書きは保持されます）"
            }
        }
    }

    fn monitor_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            line(ui, RichText::new("状態").strong());
            let link = match self.chat.bridge() {
                Bridge::Connected => "jcode接続設定済み（送信ごとに再確認・AIの応答はチャットごとに確認）",
                Bridge::Connecting => "jcodeブリッジに接続中",
                Bridge::Disconnected { .. } => "未接続",
            };
            row(ui, Prov::Fact, &format!("チャット接続: {link}"));
            row(ui, Prov::Fact, &work_target_status(&self.work_target));
            if let Some(plan) = &self.work_plan {
                let dir = display_path(&plan.working_dir);
                let perm = plan.permission.label();
                row(ui, Prov::Plan, &format!("計画（未実行）: {perm}の作業セッション · {dir}"));
            }
            row(ui, Prov::Fact, CHAT_RESTRICTION);
            self.work_target_ui(ui);
            ui.add_space(4.0);
            row(ui, Prov::Fact, "この画面の履歴はメモリ内のみ（このアプリはファイルに保存しません）");
            row(ui, Prov::Unknown, "jcode側の保存: 不明（実会話の送信内容は保存される場合あり・閉じても削除の保証なし）");
            match &self.font {
                FontStatus::Loaded { path } => row(ui, Prov::Fact, &format!("日本語フォント: {path}")),
                FontStatus::Missing => row(ui, Prov::Fact, "Japanese font not found / 日本語フォントなし"),
            }
            let demo = self.chat.sessions().iter().flat_map(|s| &s.messages).filter(|m| matches!(m.role, Role::Demo | Role::DemoError)).count();
            if demo > 0 {
                row(ui, Prov::Demo, &format!("デモ応答: {demo}件（このウィンドウ）"));
            }
            ui.add_space(8.0);
            line(ui, RichText::new("H₂O メモリ / ワーカー / ゴール").strong());
            if let Some(err) = &self.monitor.last_error {
                Frame::new()
                    .fill(p.error_bg)
                    .stroke(Stroke::new(1.0, p.error))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.add(Label::new(RichText::new(format!("読み込めませんでした（表示中の内容は変わりません）: {err}")).color(p.error)).wrap());
                    });
            }
            // ordinary chat is not an H₂O/Jev/Swarm connection; it never marks these live
            row(ui, Prov::Unknown, "H₂O: 不明（チャット接続とは別）");
            row(ui, Prov::Unknown, "Jev: 不明（使用しません）");
            row(ui, Prov::Unknown, "Swarm: 不明（チャット接続とは別）");
            match &self.monitor.snapshot {
                None => {
                    row(ui, Prov::Unknown, "メモリ: 不明（未接続）");
                    row(ui, Prov::Unknown, "ワーカー: 不明（未接続）");
                    row(ui, Prov::Unknown, "ゴール: 不明（未接続）");
                    ui.add(Label::new(RichText::new("下でスナップショットを明示的に取り込むと表示します（読み取り専用）。").small().color(p.muted)).wrap());
                }
                Some(view) => {
                    row(ui, Prov::Imported, &format!("取込・読み取り専用: {}", view.source));
                    row(
                        ui,
                        Prov::Imported,
                        &format!("取得時刻: {}", view.captured_at.as_deref().unwrap_or("不明（記載なし）")),
                    );
                    line(ui, RichText::new("取込データは現在の実状態とは限りません").small().color(p.muted));
                    if view.branches.is_empty() {
                        row(ui, Prov::Imported, "ブランチ: 0件");
                    }
                    for b in &view.branches {
                        branch_ui(ui, b);
                    }
                }
            }
            ui.add_space(10.0);
            self.import_ui(ui);
        });
    }

    /// 作業先: explicit path + explicit permission + 確定. Confirming validates and records a plan
    /// only; nothing is started and the chat session is not affected.
    fn work_target_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        line(ui, RichText::new("作業先").strong());
        ui.add(
            Label::new(
                RichText::new("フォルダの絶対パスを入力し、権限を選んで確定します。確定は確認と計画だけで、作業セッションは開始せず、このチャットのツールも無効のままです。")
                    .small()
                    .color(p.muted),
            )
            .wrap(),
        );
        if ui
            .add(
                TextEdit::singleline(&mut self.work_input)
                    .hint_text("作業先フォルダの絶対パス（例: D:\\work\\project）")
                    .desired_width(ui.available_width()),
            )
            .changed()
        {
            self.work_error = None;
        }
        // identical values keep the confirmation, so this per-frame write-back is safe
        self.work_target
            .set_path(self.work_input.trim().trim_matches('"'));
        let chosen = self.work_target.permission();
        ui.horizontal(|ui| {
            for perm in [Permission::ReadOnly, Permission::ReadWrite] {
                if ui
                    .add(egui::Button::selectable(chosen == Some(perm), perm.label()))
                    .clicked()
                {
                    self.work_target.set_permission(perm);
                    self.work_error = None;
                }
            }
            let ready = chosen.is_some() && !self.work_target.path_input().trim().is_empty();
            if ui
                .add_enabled(ready, egui::Button::new("確定"))
                .on_disabled_hover_text("パスを入力し、権限を選ぶと確定できます")
                .clicked()
            {
                self.confirm_work_target();
            }
        });
        if self.work_target.state() != WorkTargetState::Confirmed {
            self.work_plan = None;
        }
        if let Some(e) = self.work_error {
            line(
                ui,
                RichText::new(format!("作業先エラー: {e}")).color(p.error),
            );
        }
    }

    fn confirm_work_target(&mut self) {
        self.work_plan = None;
        let result = match self.work_target.confirm() {
            Ok(_) => self.work_target.plan(),
            Err(e) => Err(e),
        };
        match result {
            Ok(plan) => {
                self.work_plan = Some(plan);
                self.work_error = None;
            }
            Err(e) => self.work_error = Some(e),
        }
    }

    fn import_ui(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        line(
            ui,
            RichText::new("スナップショットの取り込み（読み取り専用）").strong(),
        );
        ui.add(
            Label::new(
                RichText::new("自動では何も読みません。h2o-snapshot/1 形式・1 MiB以下のJSONを、ファイルパスか貼り付けで明示的に指定します。")
                    .small()
                    .color(p.muted),
            )
            .wrap(),
        );
        ui.add(
            TextEdit::singleline(&mut self.path_input)
                .hint_text("ファイルパス（例: D:\\data\\snapshot.json）")
                .desired_width(ui.available_width()),
        );
        if ui.button("ファイルを読み込む").clicked() {
            let path = self.path_input.trim().trim_matches('"').to_owned();
            if !path.is_empty() {
                let ok = self.monitor.import_file(Path::new(&path));
                self.notice = ok.then(|| "ファイルを取り込みました".into());
            }
        }
        ui.add(
            TextEdit::multiline(&mut self.paste_input)
                .hint_text("ここにJSONを貼り付け")
                .desired_rows(4)
                .char_limit(crate::ui::MAX_SNAPSHOT_BYTES + 1)
                .font(TextStyle::Monospace)
                .desired_width(ui.available_width()),
        );
        ui.horizontal(|ui| {
            if ui.button("貼り付けを検証して読み込む").clicked() {
                let ok = self.monitor.import_pasted(&self.paste_input);
                self.notice = ok.then(|| "貼り付けを取り込みました".into());
            }
            if ui.button("消去").clicked() {
                self.paste_input.clear();
            }
        });
        if let Some(n) = &self.notice {
            line(ui, RichText::new(n.as_str()).small().color(p.ok));
        }
    }
}

const DEMO_ERROR_HINT: &str = "エラー表示を確認（/demo-error）";

fn branch_ui(ui: &mut egui::Ui, b: &BranchRow) {
    let p = pal(ui);
    ui.add_space(6.0);
    Frame::new()
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            line(
                ui,
                RichText::new(one_line(&format!("ブランチ {}", b.branch_id), 80)).strong(),
            );
            ui.add(Label::new(format!("ゴール: {}", prose(&b.purpose))).wrap());
            row(ui, Prov::Imported, &format!("run: {}", b.run_id));
            row(
                ui,
                Prov::Imported,
                &format!("ワーカー: {} ← {}", b.session_id, b.owner_session_id),
            );
            row(
                ui,
                Prov::Imported,
                &format!("照合要否: {}", tri(b.needs_reconciliation)),
            );
            row(
                ui,
                Prov::Imported,
                &format!("修正上限: {}", b.max_corrections),
            );
            for (name, items) in [
                ("制約", &b.constraints),
                ("成功条件", &b.success_conditions),
                ("担当パス", &b.owned_paths),
                ("未完了", &b.unfinished),
                ("context", &b.context_ids),
            ] {
                let joined = if items.is_empty() {
                    "なし".to_owned()
                } else {
                    items.join(" / ")
                };
                row(ui, Prov::Imported, &format!("{name}: {joined}"));
            }
            let total = b.checkpoints.len();
            line(
                ui,
                RichText::new(format!("チェックポイント {total}件"))
                    .small()
                    .color(p.muted),
            );
            for c in b
                .checkpoints
                .iter()
                .skip(total.saturating_sub(SHOWN_CHECKPOINTS))
            {
                row(
                    ui,
                    Prov::Imported,
                    &format!(
                        "#{} {} · 判定:{} · 決定:{} · 受領:{} · {}",
                        c.sequence,
                        c.tool,
                        c.signal,
                        c.decision,
                        receipt(c.receipt),
                        c.reason
                    ),
                );
            }
            if total > SHOWN_CHECKPOINTS {
                line(
                    ui,
                    RichText::new(format!("古い{}件は表示を省略", total - SHOWN_CHECKPOINTS))
                        .small()
                        .color(p.muted),
                );
            }
        });
}

impl Drop for NativeApp {
    fn drop(&mut self) {
        self.disconnect();
    }
}
