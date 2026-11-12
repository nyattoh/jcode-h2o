#![cfg(feature = "native-ui")]
//! Light/dark theme of the native reception UI, checked on real headless egui frames: the painted
//! rect fills and text colours (not just a flag), the visible switch, and that switching keeps all
//! chat/draft/pending/import/tab state. The OS is reported as DARK in every frame, so a LIGHT
//! default proves independence from the system theme. No window, daemon, network or keys.

use eframe::egui::{
    self, Color32, Event, Key, Modifiers, PointerButton, Rect, Shape, Theme, pos2, vec2,
};
use h2o_drift_prototype::native::{DEMO_DELAY_SECS, DEMO_ERROR_COMMAND, Layout};
use h2o_drift_prototype::native_ui::{NativeApp, install_japanese_font};
use serde_json::{Value, json};

// ---- fixtures --------------------------------------------------------------------------------

fn snapshot(run_id: &str) -> String {
    let goal = json!({"purpose": "目的の説明", "constraints": ["読み取り専用"],
        "success_conditions": ["不明が保たれる"], "owned_paths": ["experiments/h2o-drift"],
        "unfinished": []});
    let cp = |sequence: u64, signal: &str| -> Value {
        json!({"sequence": sequence,
            "input": {"goal": goal, "observation": {"call_id": format!("c{sequence}"),
                "tool": "bash", "output": "PRIVATE-OUT", "error": "PRIVATE-ERR"},
                "context": [{"id": "src", "bytes": [1]}]},
            "assessment": {"signal": signal, "reason": "理由"}, "decision": "Hold", "action": "Hold"})
    };
    json!({"schema": "h2o-snapshot/1", "captured_at": "2026-11-12T00:00:00Z",
        "branches": [{"spec": {"run_id": run_id, "branch_id": "branch-a",
            "owner_session_id": "root", "session_id": "worker-a", "goal": goal,
            "context_ids": ["src"], "max_corrections": 1},
            "checkpoints": [cp(1, "Unknown"), cp(2, "Misaligned")],
            "receipts": [{"sequence": 1, "receipt": null}], "needs_reconciliation": null}]})
    .to_string()
}

// ---- headless egui harness -------------------------------------------------------------------

struct Harness {
    ctx: egui::Context,
    app: NativeApp,
    time: f64,
    size: egui::Vec2,
}

#[derive(Clone, Debug)]
struct Text {
    text: String,
    rect: Rect,
    rows: usize,
    color: Color32,
    /// fill of the last opaque rect painted under the text's centre (its real backdrop)
    backdrop: Color32,
}

struct Frame {
    fills: Vec<(Rect, Color32)>,
    texts: Vec<Text>,
    /// accessible name and toggled state (egui maps `selected` to accesskit `toggled`)
    a11y: Vec<(String, Option<bool>)>,
}

impl Frame {
    fn text(&self, needle: &str) -> &Text {
        let exact = self.texts.iter().find(|t| t.text == needle);
        exact
            .or_else(|| self.texts.iter().find(|t| t.text.contains(needle)))
            .unwrap_or_else(|| panic!("{needle:?} not painted; painted: {}", self.joined()))
    }
    fn joined(&self) -> String {
        self.texts
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    }
    /// the panel background: the largest opaque fill
    fn bg(&self) -> Color32 {
        self.fills
            .iter()
            .max_by(|a, b| a.0.area().total_cmp(&b.0.area()))
            .expect("some fill")
            .1
    }
}

impl Harness {
    fn new(width: f32, height: f32) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let font = install_japanese_font(&ctx);
        let mut h = Self {
            ctx,
            app: NativeApp::new(font),
            time: 0.0,
            size: vec2(width, height),
        };
        h.frame(vec![]);
        h.frame(vec![]); // focus requests land on the following pass
        h
    }

    fn frame(&mut self, events: Vec<Event>) -> Frame {
        self.time += 0.016;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)),
            time: Some(self.time),
            system_theme: Some(Theme::Dark), // the OS asks for dark; the app must still start light
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear(); // no renderer in headless tests
        let mut fills = Vec::new();
        let mut texts = Vec::new();
        fn walk(shape: &Shape, fills: &mut Vec<(Rect, Color32)>, texts: &mut Vec<Text>) {
            match shape {
                Shape::Rect(r) if r.fill.a() == 255 && r.rect.area() > 0.0 => {
                    fills.push((r.rect, r.fill))
                }
                Shape::Text(t) if !t.galley.text().trim().is_empty() => {
                    let section = t.galley.job.sections.first().map(|s| s.format.color);
                    let color = t.override_text_color.unwrap_or(match section {
                        Some(c) if c != Color32::PLACEHOLDER => c,
                        _ => t.fallback_color,
                    });
                    let rect = t.visual_bounding_rect();
                    let backdrop = fills
                        .iter()
                        .rev()
                        .find(|(r, _)| r.contains(rect.center()))
                        .map_or(Color32::TRANSPARENT, |f| f.1);
                    texts.push(Text {
                        text: t.galley.text().to_owned(),
                        rect,
                        rows: t.galley.rows.len(),
                        color,
                        backdrop,
                    });
                }
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, fills, texts)),
                _ => {}
            }
        }
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut fills, &mut texts);
        }
        let a11y = out
            .platform_output
            .accesskit_update
            .map(|u| {
                u.nodes
                    .iter()
                    .filter_map(|(_, n)| {
                        n.label().map(|l| {
                            (
                                l.to_owned(),
                                n.toggled().map(|t| t == egui::accesskit::Toggled::True),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Frame { fills, texts, a11y }
    }

    fn wait(&mut self, secs: f64) -> Frame {
        self.time += secs;
        self.frame(vec![])
    }

    /// A real pointer click on the painted text, then the pointer leaves (no hover tint).
    fn click(&mut self, label: &str) -> Frame {
        let f = self.frame(vec![]);
        let at = f
            .texts
            .iter()
            .find(|t| t.text == label)
            .unwrap_or_else(|| panic!("no clickable {label:?}; painted: {}", f.joined()))
            .rect
            .center();
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at), button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![Event::PointerGone]);
        self.frame(vec![]) // theme changes may take one more pass to reach every panel
    }
}

fn enter() -> Event {
    Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

// ---- colour maths (WCAG 2.x) -----------------------------------------------------------------

fn over(c: Color32, bg: Color32) -> Color32 {
    // premultiplied source-over onto an opaque backdrop
    let k = 1.0 - c.a() as f32 / 255.0;
    let ch = |s: u8, b: u8| (s as f32 + b as f32 * k).round().min(255.0) as u8;
    Color32::from_rgb(ch(c.r(), bg.r()), ch(c.g(), bg.g()), ch(c.b(), bg.b()))
}

fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn text_contrast(t: &Text, bg: Color32) -> f32 {
    let back = if t.backdrop == Color32::TRANSPARENT {
        bg
    } else {
        t.backdrop
    };
    contrast(over(t.color, back), back)
}

/// Every painted text is readable on its actual backdrop; disabled controls may be dimmer.
fn assert_readable(f: &Frame, theme: &str) {
    let bg = f.bg();
    for t in &f.texts {
        let min = if t.text == "送信" { 2.0 } else { 3.0 };
        let c = text_contrast(t, bg);
        assert!(
            c >= min,
            "{theme}: {:?} contrast {c:.2} < {min} (text {:?} on {:?})",
            t.text,
            t.color,
            t.backdrop
        );
    }
}

/// Every chat/status/selection/error state is on screen at once: user, demo and demo-error
/// bubbles; a pending reply (loading + disabled send); a selected history row; the imported
/// snapshot with its chips; the import error frame; the warning banner.
fn populate(h: &mut Harness) {
    h.frame(vec![Event::Text("こんにちは".into())]);
    h.frame(vec![enter()]);
    h.wait(DEMO_DELAY_SECS + 0.1);
    h.frame(vec![Event::Text(DEMO_ERROR_COMMAND.into())]);
    h.frame(vec![enter()]);
    h.wait(DEMO_DELAY_SECS + 0.1);
    h.frame(vec![Event::Text("保留中の質問".into())]);
    h.frame(vec![enter()]);
    assert!(h.app.chat().active().unwrap().is_loading());
    assert!(h.app.monitor_mut().import_pasted(&snapshot("run-1")));
    assert!(!h.app.monitor_mut().import_pasted("{}"));
}

const STATES: [&str; 9] = [
    "デモ・未接続",
    "あなた",
    "デモ応答（未接続）",
    "デモのエラー（擬似）",
    "生成中",
    "送信",
    "取込",
    "不明",
    "読み込めませんでした",
];

// ---- tests -----------------------------------------------------------------------------------

#[test]
fn default_render_is_light_even_when_os_is_dark_and_every_state_uses_the_light_palette() {
    let mut h = Harness::new(1280.0, 1400.0);
    populate(&mut h);
    let f = h.frame(vec![]);
    let bg = f.bg();
    assert!(
        luminance(bg) > 0.8,
        "default background is not light: {bg:?}"
    );
    assert_eq!(
        h.ctx.theme(),
        Theme::Light,
        "egui theme must default to light"
    );
    for (rect, fill) in &f.fills {
        assert!(
            luminance(*fill) > 0.45,
            "dark fill left in light mode: {fill:?} at {rect:?}"
        );
    }
    for needle in STATES {
        let t = f.text(needle);
        assert!(
            luminance(over(t.color, t.backdrop)) < 0.3,
            "light-mode {needle:?} text is not dark ink: {:?}",
            t.color
        );
    }
    assert_readable(&f, "light");
    // selected history row is visibly distinct but still light
    let sel = f.text("こんにちは …");
    assert_ne!(sel.backdrop, bg, "selected row not highlighted");
    assert!(luminance(sel.backdrop) > 0.45, "{:?}", sel.backdrop);
}

#[test]
fn theme_switch_paints_dark_palette_and_back_without_losing_any_state() {
    let mut h = Harness::new(1280.0, 1400.0);
    populate(&mut h);
    h.frame(vec![Event::Text("下書き".into())]);
    let light = h.frame(vec![]);
    assert!(
        luminance(light.bg()) > 0.8,
        "must start light: {:?}",
        light.bg()
    );
    assert_eq!(h.ctx.theme(), Theme::Light, "must start light");
    let before = (
        format!("{:?}", h.app.chat().sessions()),
        h.app.chat().active_id(),
        h.app.composer().to_owned(),
        format!("{:?}", h.app.monitor_mut().snapshot),
        h.app.monitor_mut().last_error.clone(),
    );

    // the switch is visible, Japanese, and exposed to assistive tech with its selected state
    assert!(
        light.a11y.contains(&("ライト".into(), Some(true))),
        "{:?}",
        light.a11y
    );
    assert!(
        light
            .a11y
            .iter()
            .any(|(l, s)| l == "ダーク" && *s == Some(false)),
        "{:?}",
        light.a11y
    );

    let dark = h.click("ダーク");
    assert_eq!(h.ctx.theme(), Theme::Dark);
    assert!(luminance(dark.bg()) < 0.05, "dark bg: {:?}", dark.bg());
    for (rect, fill) in &dark.fills {
        assert!(
            luminance(*fill) < 0.2,
            "light fill in dark mode: {fill:?} {rect:?}"
        );
    }
    for needle in STATES {
        let (l, d) = (light.text(needle), dark.text(needle));
        assert!(
            luminance(over(d.color, d.backdrop)) > 0.3,
            "dark-mode {needle:?} text not light ink: {:?}",
            d.color
        );
        assert_ne!(
            (l.color, l.backdrop),
            (d.color, d.backdrop),
            "{needle:?} paint ignores the theme"
        );
    }
    assert_readable(&dark, "dark");
    assert!(dark.a11y.contains(&("ダーク".into(), Some(true))));

    // pending reply survived the switch and still resolves
    assert!(h.app.chat().active().unwrap().is_loading());
    let after = (
        format!("{:?}", h.app.chat().sessions()),
        h.app.chat().active_id(),
        h.app.composer().to_owned(),
        format!("{:?}", h.app.monitor_mut().snapshot),
        h.app.monitor_mut().last_error.clone(),
    );
    assert_eq!(before, after, "switching to dark changed state");
    assert_eq!(h.app.composer(), "下書き");

    let back = h.click("ライト");
    assert_eq!(h.ctx.theme(), Theme::Light);
    let paints = |f: &Frame| {
        STATES
            .iter()
            .map(|n| (f.text(n).color, f.text(n).backdrop))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        paints(&light),
        paints(&back),
        "toggle back is not the same light"
    );
    assert_eq!(light.bg(), back.bg());
    assert_eq!(h.app.composer(), "下書き");
    h.wait(DEMO_DELAY_SECS + 0.1);
    let s = h.app.chat().active().unwrap();
    assert!(!s.is_loading(), "pending reply lost by theme switching");
    assert_eq!(s.messages.len(), 6);
}

#[test]
fn narrow_switch_is_visible_one_row_and_keeps_monitor_tab_import_and_error() {
    let mut h = Harness::new(360.0, 1400.0);
    assert_eq!(h.app.layout(), Layout::Narrow);
    h.frame(vec![Event::Text("送る前の下書き".into())]);
    assert!(h.app.monitor_mut().import_pasted(&snapshot("run-narrow")));
    assert!(!h.app.monitor_mut().import_pasted("not json"));
    let err = h.app.monitor_mut().last_error.clone();
    h.app.show_monitor_pane();
    let f = h.frame(vec![]);
    assert!(luminance(f.bg()) > 0.8, "narrow default not light");
    for label in ["ライト", "ダーク"] {
        let t = f.text(label);
        assert_eq!(t.rows, 1, "{label} wraps");
        assert!(
            t.rect.right() <= 360.5 && t.rect.left() >= 0.0,
            "{label} off-screen {:?}",
            t.rect
        );
    }
    for t in &f.texts {
        assert!(
            t.rect.right() <= 360.5,
            "overflow: {:?} {:?}",
            t.text,
            t.rect
        );
    }
    assert!(f.joined().contains("run-narrow"), "monitor tab content");

    let d = h.click("ダーク");
    assert!(luminance(d.bg()) < 0.05);
    assert!(
        d.joined().contains("run-narrow"),
        "monitor tab lost: {}",
        d.joined()
    );
    assert!(d.joined().contains("読み込めませんでした"));
    assert_eq!(h.app.monitor_mut().last_error, err);
    assert_eq!(h.app.composer(), "送る前の下書き");
    assert!(d.text("ライト").rect.right() <= 360.5);
    assert_readable(&d, "narrow dark");

    let l = h.click("ライト");
    assert!(luminance(l.bg()) > 0.8);
    assert!(l.joined().contains("run-narrow"));
    assert_eq!(h.app.composer(), "送る前の下書き");
    assert_readable(&l, "narrow light");
}
