//! egui view of the native reception UI. State and decisions live in [`crate::native`]; this file
//! only lays out widgets, maps egui input to [`KeyInput`] and loads a system Japanese font.
//! It never connects to anything and never writes files.
use crate::native::{
    BranchRow, Chat, ImeGuard, KeyInput, Layout, Monitor, ReceiptState, Role, Tri, layout_for,
    one_line, prose,
};
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Key,
    KeyboardShortcut, Label, Margin, Modifiers, RichText, ScrollArea, Stroke, TextEdit, TextStyle,
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

const BG: Color32 = Color32::from_rgb(0x0e, 0x10, 0x14);
const SIDEBAR: Color32 = Color32::from_rgb(0x15, 0x17, 0x1c);
const SURFACE: Color32 = Color32::from_rgb(0x1c, 0x1f, 0x26);
const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x37);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xec);
const MUTED: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xab);
const ACCENT: Color32 = Color32::from_rgb(0x8a, 0xb4, 0xf8);
const WARN: Color32 = Color32::from_rgb(0xf2, 0xb8, 0x4b);
const ERROR: Color32 = Color32::from_rgb(0xf2, 0x8b, 0x82);
const OK: Color32 = Color32::from_rgb(0x81, 0xc9, 0x95);

fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE;
    v.faint_bg_color = SIDEBAR;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(0x2b, 0x3d, 0x5c);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = SURFACE;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x26, 0x2a, 0x33);
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
    ctx.set_global_style(style);
}

// ---- provenance ------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Prov {
    Fact,
    Demo,
    Unknown,
    Imported,
}

impl Prov {
    fn chip(self) -> (&'static str, Color32) {
        match self {
            Prov::Fact => ("事実", OK),
            Prov::Demo => ("デモ", WARN),
            Prov::Unknown => ("不明", MUTED),
            Prov::Imported => ("取込", ACCENT),
        }
    }
}

/// One compact status row: provenance chip + a single truncated line.
fn row(ui: &mut egui::Ui, prov: Prov, text: &str) {
    ui.horizontal(|ui| {
        let (label, color) = prov.chip();
        Frame::new()
            .stroke(Stroke::new(1.0, color))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(4, 0))
            .show(ui, |ui| ui.label(RichText::new(label).small().color(color)));
        ui.add(Label::new(one_line(text, 300)).truncate());
    });
}

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
    chat: Chat,
    monitor: Monitor,
    composer: String,
    guard: ImeGuard,
    font: FontStatus,
    layout: Layout,
    pane: Pane,
    monitor_open: bool,
    focus_composer: bool,
    styled: bool,
    path_input: String,
    paste_input: String,
    notice: Option<String>,
}

impl eframe::App for NativeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl NativeApp {
    pub fn new(font: FontStatus) -> Self {
        Self {
            chat: Chat::default(),
            monitor: Monitor::default(),
            composer: String::new(),
            guard: ImeGuard::default(),
            font,
            layout: Layout::Wide,
            pane: Pane::Chat,
            monitor_open: false,
            focus_composer: true,
            styled: false,
            path_input: String::new(),
            paste_input: String::new(),
            notice: None,
        }
    }

    pub fn chat(&self) -> &Chat {
        &self.chat
    }
    pub fn composer(&self) -> &str {
        &self.composer
    }
    pub fn layout(&self) -> Layout {
        self.layout
    }
    pub fn monitor_mut(&mut self) -> &mut Monitor {
        &mut self.monitor
    }

    pub fn show_monitor_pane(&mut self) {
        self.pane = Pane::Monitor;
        self.monitor_open = true;
    }

    pub fn new_chat(&mut self) {
        self.chat.new_session();
        self.pane = Pane::Chat;
        self.focus_composer = true;
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
        if !self.styled {
            apply_theme(ui.ctx());
            self.styled = true;
        }
        let now = ui.input(|i| i.time);
        self.chat.tick(now);
        if let Some(due) = self.chat.next_due() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
        self.layout = layout_for(ui.available_width());

        egui::Panel::top("header")
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| self.header(ui));

        match self.layout {
            Layout::Narrow => {
                egui::Panel::top("tabs")
                    .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(12, 4)))
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
                    .frame(Frame::new().fill(BG).inner_margin(Margin::same(10)))
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
                    .frame(Frame::new().fill(SIDEBAR).inner_margin(Margin::same(12)))
                    .show(ui, |ui| self.history_ui(ui));
                if layout == Layout::Wide || self.monitor_open {
                    egui::Panel::right("monitor")
                        .resizable(true)
                        .default_size(340.0)
                        .size_range(260.0..=520.0)
                        .frame(Frame::new().fill(SIDEBAR).inner_margin(Margin::same(12)))
                        .show(ui, |ui| self.monitor_ui(ui));
                }
                egui::CentralPanel::default()
                    .frame(
                        Frame::new()
                            .fill(BG)
                            .inner_margin(Margin::symmetric(16, 10)),
                    )
                    .show(ui, |ui| self.chat_ui(ui));
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(Label::new(RichText::new("H₂O 受付").strong().size(17.0)).truncate());
            if self.layout == Layout::Medium {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.monitor_open {
                        "監視パネルを閉じる"
                    } else {
                        "監視パネル"
                    };
                    if ui.button(label).clicked() {
                        self.monitor_open = !self.monitor_open;
                    }
                });
            }
        });
        Frame::new()
            .fill(Color32::from_rgb(0x2a, 0x22, 0x10))
            .stroke(Stroke::new(1.0, WARN))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                line(
                    ui,
                    RichText::new(
                        "デモ・未接続 — モデル/Jev/daemonには接続しません。応答は固定のデモです。",
                    )
                    .color(WARN),
                );
                line(
                    ui,
                    RichText::new(
                        "作業対象: 未確認 · 書き込み権限なし（チャットから編集・実行しません）",
                    )
                    .color(WARN)
                    .small(),
                );
            });
    }

    fn history_ui(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_width();
        if ui
            .add_sized(
                [full, 34.0],
                egui::Button::new("＋ 新しいチャット").fill(SURFACE),
            )
            .clicked()
        {
            self.new_chat();
        }
        ui.add_space(6.0);
        line(ui, RichText::new("履歴").color(MUTED).small());
        let mut clicked = None;
        egui::Panel::bottom("history-scope")
            .frame(Frame::new().inner_margin(Margin::symmetric(0, 6)))
            .show(ui, |ui| {
                line(
                    ui,
                    RichText::new("履歴は保存されません（メモリ内のみ）")
                        .color(MUTED)
                        .small(),
                );
            });
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.chat.sessions().is_empty() {
                    line(ui, RichText::new("履歴はまだありません").color(MUTED));
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
        egui::Panel::bottom("composer")
            .frame(Frame::new().inner_margin(Margin::symmetric(0, 8)))
            .show(ui, |ui| self.composer_ui(ui));
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
                                Role::User => ("あなた", SURFACE, MUTED),
                                Role::Demo => ("デモ応答（未接続）", BG, WARN),
                                Role::DemoError => (
                                    "デモのエラー（擬似）",
                                    Color32::from_rgb(0x2c, 0x16, 0x16),
                                    ERROR,
                                ),
                            };
                            ui.add_space(4.0);
                            line(ui, RichText::new(who).small().color(color));
                            Frame::new()
                                .fill(fill)
                                .stroke(Stroke::new(
                                    1.0,
                                    if m.role == Role::User { BORDER } else { color },
                                ))
                                .corner_radius(CornerRadius::same(12))
                                .inner_margin(Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.set_max_width(ui.available_width());
                                    ui.add(Label::new(prose(&m.text)).wrap());
                                });
                        }
                        if s.is_loading() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                line(ui, RichText::new("デモ応答を生成中…").color(MUTED));
                            });
                        }
                    });
            });
    }

    fn welcome_ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.25).min(140.0));
            ui.add(Label::new(RichText::new("何をお手伝いしましょう？").heading()).wrap());
            ui.add_space(6.0);
            ui.add(
                Label::new(
                    RichText::new("これはデモです。送信内容はどこにも送られず、決まった形のデモ応答が返ります。")
                        .color(MUTED),
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

    fn composer_ui(&mut self, ui: &mut egui::Ui) {
        let loading = self.chat.active().is_some_and(|s| s.is_loading());
        let id = ui.make_persistent_id("composer");
        let was_focused = ui.memory(|m| m.has_focus(id));
        let mut send = false;
        Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let button_w = 64.0;
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
                    let can_send = !loading && !self.composer.trim().is_empty();
                    let b = ui.add_enabled(
                        can_send,
                        egui::Button::new("送信").min_size(egui::vec2(button_w, 32.0)),
                    );
                    if b.clicked() {
                        send = true;
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
        if send {
            let now = ui.input(|i| i.time);
            if self.chat.send(self.composer.trim(), now) {
                self.composer.clear();
                self.pane = Pane::Chat;
            }
            self.focus_composer = true;
        }
        let hint = if loading {
            "デモ応答の生成中は送信できません"
        } else {
            "Enterで送信 · Shift+Enterで改行 · 変換中のEnterは送信しません"
        };
        line(ui, RichText::new(hint).small().color(MUTED));
    }

    fn monitor_ui(&mut self, ui: &mut egui::Ui) {
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            line(ui, RichText::new("状態").strong());
            row(ui, Prov::Fact, "接続: 未接続（モデル・Jev・daemonなし）");
            row(ui, Prov::Fact, "作業対象: 未確認 · 書き込み権限なし");
            row(ui, Prov::Fact, "履歴: このウィンドウのメモリ内のみ");
            match &self.font {
                FontStatus::Loaded { path } => row(ui, Prov::Fact, &format!("日本語フォント: {path}")),
                FontStatus::Missing => row(ui, Prov::Fact, "Japanese font not found / 日本語フォントなし"),
            }
            let demo = self.chat.sessions().iter().flat_map(|s| &s.messages).filter(|m| m.role != Role::User).count();
            row(ui, Prov::Demo, &format!("デモ応答: {demo}件（このウィンドウ）"));
            ui.add_space(8.0);
            line(ui, RichText::new("H₂O メモリ / ワーカー / ゴール").strong());
            if let Some(err) = &self.monitor.last_error {
                Frame::new()
                    .stroke(Stroke::new(1.0, ERROR))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.add(Label::new(RichText::new(format!("読み込めませんでした（表示中の内容は変わりません）: {err}")).color(ERROR)).wrap());
                    });
            }
            match &self.monitor.snapshot {
                None => {
                    row(ui, Prov::Unknown, "メモリ: 不明（未接続）");
                    row(ui, Prov::Unknown, "ワーカー: 不明（未接続）");
                    row(ui, Prov::Unknown, "ゴール: 不明（未接続）");
                    ui.add(Label::new(RichText::new("下でスナップショットを明示的に取り込むと表示します（読み取り専用）。").small().color(MUTED)).wrap());
                }
                Some(view) => {
                    row(ui, Prov::Imported, &format!("取込・読み取り専用: {}", view.source));
                    row(
                        ui,
                        Prov::Imported,
                        &format!("取得時刻: {}", view.captured_at.as_deref().unwrap_or("不明（記載なし）")),
                    );
                    line(ui, RichText::new("取込データは現在の実状態とは限りません").small().color(MUTED));
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

    fn import_ui(&mut self, ui: &mut egui::Ui) {
        line(
            ui,
            RichText::new("スナップショットの取り込み（読み取り専用）").strong(),
        );
        ui.add(
            Label::new(
                RichText::new("自動では何も読みません。h2o-snapshot/1 形式・1 MiB以下のJSONを、ファイルパスか貼り付けで明示的に指定します。")
                    .small()
                    .color(MUTED),
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
            line(ui, RichText::new(n.as_str()).small().color(OK));
        }
    }
}

const DEMO_ERROR_HINT: &str = "エラー表示を確認（/demo-error）";

fn branch_ui(ui: &mut egui::Ui, b: &BranchRow) {
    ui.add_space(6.0);
    Frame::new()
        .stroke(Stroke::new(1.0, BORDER))
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
                    .color(MUTED),
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
                        .color(MUTED),
                );
            }
        });
}
