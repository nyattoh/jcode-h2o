#![cfg(feature = "native-ui")]
//! Real chat in the native UI, driven through real headless egui frames (pointer clicks, typed
//! text, Enter, IME events) into the REAL `ChatController` and a REAL `JcodeClient`, connected to
//! the scripted in-memory harness peer of `tests/chat_support`. No window, daemon, provider,
//! model, Jev, key or network is touched; the peer records every request the UI caused.

use eframe::egui::{
    self, Color32, Event, ImeEvent, Key, Modifiers, PointerButton, Rect, Shape, Theme, pos2, vec2,
};
use h2o_drift_prototype::native::{Kind, Role};
use h2o_drift_prototype::native_ui::{NativeApp, install_japanese_font};
use jcode_sdk::{ApiEvent, ApiRequest};
use serde_json::json;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

mod chat_support;
use chat_support::*;

// ---- headless egui harness -------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Text {
    text: String,
    rect: Rect,
    rows: usize,
    color: Color32,
    backdrop: Color32,
    /// The clip rectangle it was painted with (scroll areas, panels).
    clip: Rect,
}

struct Frame {
    fills: Vec<(Rect, Color32)>,
    texts: Vec<Text>,
}

impl Frame {
    fn joined(&self) -> String {
        self.texts
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    }
    fn has(&self, needle: &str) -> bool {
        self.texts.iter().any(|t| t.text.contains(needle))
    }
    fn text(&self, needle: &str) -> &Text {
        let exact = self.texts.iter().find(|t| t.text == needle);
        exact
            .or_else(|| self.texts.iter().find(|t| t.text.contains(needle)))
            .unwrap_or_else(|| panic!("{needle:?} not painted; painted: {}", self.joined()))
    }
    fn bg(&self) -> Color32 {
        self.fills
            .iter()
            .max_by(|a, b| a.0.area().total_cmp(&b.0.area()))
            .expect("some fill")
            .1
    }
}

struct Gui {
    ctx: egui::Context,
    app: NativeApp,
    time: f64,
    size: egui::Vec2,
}

impl Gui {
    fn new(peer: &Harness, width: f32, height: f32) -> Self {
        let ctx = egui::Context::default();
        let font = install_japanese_font(&ctx);
        let mut g = Self {
            ctx,
            app: NativeApp::with_controller(font, peer.controller()),
            time: 0.0,
            size: vec2(width, height),
        };
        g.frame(vec![]);
        g.frame(vec![]);
        g
    }

    fn frame(&mut self, events: Vec<Event>) -> Frame {
        self.time += 0.016;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)),
            time: Some(self.time),
            system_theme: Some(Theme::Dark),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
        let mut fills = Vec::new();
        let mut texts = Vec::new();
        fn walk(
            shape: &Shape,
            clip: Rect,
            fills: &mut Vec<(Rect, Color32)>,
            texts: &mut Vec<Text>,
        ) {
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
                        clip,
                    });
                }
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, fills, texts)),
                _ => {}
            }
        }
        for clipped in &out.shapes {
            walk(&clipped.shape, clipped.clip_rect, &mut fills, &mut texts);
        }
        Frame { fills, texts }
    }

    /// A real pointer click on painted text (exact match first, then containment).
    fn click(&mut self, label: &str) -> Frame {
        let f = self.frame(vec![]);
        let at = f.text(label).rect.center();
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at), button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![Event::PointerGone]);
        self.frame(vec![])
    }

    fn type_text(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
    }

    /// Frames in real time (the controller works on its own threads) until `pred` holds.
    fn until(
        &mut self,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&Frame, &NativeApp) -> bool,
    ) -> Frame {
        let deadline = Instant::now() + timeout;
        loop {
            let started = Instant::now();
            let f = self.frame(vec![]);
            assert!(
                started.elapsed() < Duration::from_millis(250),
                "a frame blocked for {:?}",
                started.elapsed()
            );
            if pred(&f, &self.app) {
                return f;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; painted: {}",
                f.joined()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Frames for `period` of real time; returns the last.
    fn idle(&mut self, period: Duration) -> Frame {
        let deadline = Instant::now() + period;
        let mut f = self.frame(vec![]);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            f = self.frame(vec![]);
        }
        f
    }

    /// Explicit user connect: click the endpoint field, type the endpoint, click 接続.
    fn connect(&mut self) -> Frame {
        self.click(ENDPOINT_HINT);
        self.type_text(TEST_ENDPOINT);
        self.click("接続");
        self.until("ready", Duration::from_secs(5), |f, _| f.has(READY))
    }

    fn focus_composer(&mut self) {
        self.click(COMPOSER_HINT);
    }

    fn roles(&self) -> Vec<Role> {
        self.app
            .chat()
            .active()
            .map(|s| s.messages.iter().map(|m| m.role).collect())
            .unwrap_or_default()
    }
}

const ENDPOINT_HINT: &str = "既存jcodeのソケットパス";
const COMPOSER_HINT: &str = "メッセージを入力";
// logical ready (C1 holds no socket between turns), formerly "接続済み"
const READY: &str = "接続設定済み";
const ASSISTANT: &str = "AI（jcode・実応答）";

fn key(k: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn enter() -> Event {
    key(Key::Enter, Modifiers::NONE)
}

fn sends(peer: &Harness) -> Vec<(String, String)> {
    peer.requests()
        .into_iter()
        .filter_map(|r| match r {
            ApiRequest::SendMessage {
                session_id,
                content,
                ..
            } => Some((session_id, content)),
            _ => None,
        })
        .collect()
}

fn position(peer: &Harness, pred: impl Fn(&ApiRequest) -> bool) -> Option<usize> {
    peer.requests().iter().position(pred)
}

fn no_demo(f: &Frame) {
    for marker in ["デモ応答", "固定のデモ", "擬似"] {
        assert!(
            !f.has(marker),
            "demo text {marker:?} in real mode: {}",
            f.joined()
        );
    }
}

/// Echo the prompt so the test can tell which reply belongs to which request.
fn echo() -> TurnScript {
    Arc::new(|session, content| reply_steps(session, &format!("echo[{session}]: {content}")))
}

// ---- colour maths (WCAG 2.x) -----------------------------------------------------------------

fn over(c: Color32, bg: Color32) -> Color32 {
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

fn assert_readable(f: &Frame, theme: &str) {
    let bg = f.bg();
    for t in &f.texts {
        // disabled buttons may be dimmer
        let min = if ["送信", "接続", "＋ 新しいチャット"].contains(&t.text.as_str()) {
            2.0
        } else {
            3.0
        };
        let back = if t.backdrop == Color32::TRANSPARENT {
            bg
        } else {
            t.backdrop
        };
        let c = contrast(over(t.color, back), back);
        assert!(
            c >= min,
            "{theme}: {:?} contrast {c:.2} < {min} ({:?} on {:?})",
            t.text,
            t.color,
            back
        );
    }
}

// ---- tests -----------------------------------------------------------------------------------

#[test]
fn default_is_unconnected_real_chat_and_nothing_is_called_without_an_explicit_connect() {
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    let f = g.frame(vec![]);
    assert!(f.has("未接続"), "{}", f.joined());
    assert!(f.has(ENDPOINT_HINT), "endpoint field: {}", f.joined());
    assert!(f.has("接続"), "connect button: {}", f.joined());
    assert!(f.has("自動起動しません"), "{}", f.joined());
    no_demo(&f);

    // typing and pressing Enter while unconnected neither sends nor discards the draft
    g.focus_composer();
    g.type_text("こんにちは");
    g.frame(vec![enter()]);
    let f = g.idle(Duration::from_millis(300));
    assert_eq!(
        g.app.composer(),
        "こんにちは",
        "draft kept while unconnected"
    );
    assert!(g.roles().is_empty(), "nothing appended: {:?}", g.roles());
    assert!(f.has("接続してください"), "{}", f.joined());
    no_demo(&f);

    // typing an endpoint is not a connection, and neither is a new chat
    g.click(ENDPOINT_HINT);
    g.type_text(TEST_ENDPOINT);
    g.click("＋ 新しいチャット");
    g.idle(Duration::from_millis(300));
    assert!(
        peer.endpoints().is_empty(),
        "connected without 接続: {:?}",
        peer.endpoints()
    );
    assert!(
        peer.requests().is_empty(),
        "requests without 接続: {:?}",
        peer.requests()
    );
}

#[test]
fn connect_then_send_shows_the_actual_sdk_reply_with_reported_route_after_tool_exclusion() {
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    let ready = g.connect();
    assert_eq!(
        peer.endpoints(),
        vec![std::path::PathBuf::from(TEST_ENDPOINT)]
    );
    assert!(
        ready.has("scripted-provider") && ready.has("scripted-model"),
        "reported route: {}",
        ready.joined()
    );
    assert!(ready.has("初回の実応答: 未確認"), "{}", ready.joined());
    assert!(
        sends(&peer).is_empty(),
        "prompt before Send: {:?}",
        sends(&peer)
    );
    let configured = position(&peer, |r| matches!(r, ApiRequest::ConfigureTools { .. }))
        .expect("tool exclusion configured before ready");

    g.focus_composer();
    g.type_text("こんにちは");
    let sent = g.frame(vec![enter()]);
    assert_eq!(g.app.composer(), "", "composer cleared on accepted send");
    assert!(sent.has("実応答を待っています"), "{}", sent.joined());
    let f = g.until("reply", Duration::from_secs(5), |f, _| {
        f.has("scripted reply")
    });
    assert!(f.has(ASSISTANT), "{}", f.joined());
    assert!(f.has("実応答: 受信済み"), "{}", f.joined());
    no_demo(&f);
    let s = g.app.chat().active().unwrap();
    assert_eq!(
        s.messages
            .iter()
            .map(|m| (m.role, m.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (Role::User, "こんにちは"),
            (Role::Assistant, "scripted reply")
        ]
    );
    assert_eq!(
        sends(&peer),
        [("sess-0".to_owned(), "こんにちは".to_owned())]
    );
    let sent_at = position(&peer, |r| matches!(r, ApiRequest::SendMessage { .. })).unwrap();
    assert!(
        configured < sent_at,
        "configure_tools must precede the prompt"
    );

    // no duplicate final reply
    g.idle(Duration::from_millis(300));
    assert_eq!(g.roles(), [Role::User, Role::Assistant]);

    // ordinary chat never marks H2O/Jev/Swarm/memory live
    g.app.show_monitor_pane();
    let m = g.frame(vec![]);
    for row in ["メモリ: 不明", "Jev: 不明", "Swarm: 不明"] {
        assert!(m.has(row), "{row}: {}", m.joined());
    }
}

#[test]
fn new_chat_gets_its_own_isolated_session_and_old_replies_stay_in_their_origin() {
    let peer = Harness::new(Script {
        turn: echo(),
        ..Script::default()
    });
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    g.connect();
    g.focus_composer();
    g.type_text("一つ目");
    g.frame(vec![enter()]);
    g.until("first reply", Duration::from_secs(5), |f, _| {
        f.has("echo[sess-0]: 一つ目")
    });
    let first = g.app.chat().active_id().unwrap();

    g.click("＋ 新しいチャット");
    let f = g.until("second session", Duration::from_secs(5), |f, app| {
        f.has(READY) && app.chat().active_id() != Some(first)
    });
    assert!(
        !f.has("echo[sess-0]"),
        "new chat shows old messages: {}",
        f.joined()
    );
    g.focus_composer();
    g.type_text("二つ目");
    g.frame(vec![enter()]);
    g.until("second reply", Duration::from_secs(5), |f, _| {
        f.has("echo[sess-1]: 二つ目")
    });
    assert_eq!(
        sends(&peer),
        [
            ("sess-0".to_owned(), "一つ目".to_owned()),
            ("sess-1".to_owned(), "二つ目".to_owned())
        ],
        "each conversation uses its own SDK session"
    );

    // the first conversation keeps exactly its own history and is not silently continued
    assert!(g.app.select_session(first));
    let f = g.frame(vec![]);
    let s = g.app.chat().active().unwrap();
    assert_eq!(
        s.messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        ["一つ目", "echo[sess-0]: 一つ目"]
    );
    assert!(f.has("新しいチャットで続けてください"), "{}", f.joined());
    g.focus_composer();
    g.type_text("続き");
    g.frame(vec![enter()]);
    g.idle(Duration::from_millis(300));
    assert_eq!(
        sends(&peer).len(),
        2,
        "closed conversation sent: {:?}",
        sends(&peer)
    );
    assert_eq!(g.app.composer(), "続き", "draft kept");
}

#[test]
fn connection_failure_is_shown_and_never_replaced_by_a_demo_reply() {
    let mut script = Script::default();
    script.capabilities.retain(|c| c != "session_tools");
    let peer = Harness::new(script);
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    g.click(ENDPOINT_HINT);
    g.type_text(TEST_ENDPOINT);
    g.click("接続");
    let f = g.until("failure", Duration::from_secs(5), |f, _| {
        f.has("接続エラー")
    });
    assert!(f.has("未接続"), "{}", f.joined());
    no_demo(&f);
    assert!(sends(&peer).is_empty());
    g.focus_composer();
    g.type_text("届かないはず");
    g.frame(vec![enter()]);
    let f = g.idle(Duration::from_millis(300));
    assert!(sends(&peer).is_empty(), "sent without tool exclusion");
    assert_eq!(g.app.composer(), "届かないはず");
    no_demo(&f);
    assert!(!g.roles().contains(&Role::Assistant));
}

#[test]
fn known_turn_failure_restores_the_draft_without_resending() {
    // a turn that ends without any assistant text: a definite failure, not a reply
    let peer = Harness::new(Script {
        turn: Arc::new(|session, _| {
            vec![
                ev(ApiEvent::MessageAccepted {
                    session_id: session.into(),
                }),
                turn_done(session),
            ]
        }),
        ..Script::default()
    });
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    g.connect();
    g.focus_composer();
    g.type_text("失敗する質問");
    g.frame(vec![enter()]);
    let f = g.until("failure notice", Duration::from_secs(5), |_, app| {
        app.chat()
            .active()
            .is_some_and(|s| s.messages.iter().any(|m| m.role == Role::Notice))
    });
    let f = if f.has("送信できませんでした") {
        f
    } else {
        g.frame(vec![])
    };
    assert!(f.has("送信できませんでした"), "{}", f.joined());
    no_demo(&f);
    assert_eq!(
        g.app.composer(),
        "失敗する質問",
        "draft restored for an explicit retry"
    );
    assert!(!g.roles().contains(&Role::Assistant));
    g.idle(Duration::from_millis(300));
    assert_eq!(sends(&peer).len(), 1, "no automatic resend");
    assert!(f.has(READY), "session stays usable: {}", f.joined());

    // the explicit retry is the user's Send (the restored draft keeps the composer focused)
    g.frame(vec![enter()]);
    g.until("second terminal", Duration::from_secs(5), |_, _| {
        sends(&peer).len() == 2
    });
}

#[test]
fn disconnect_mid_turn_is_uncertain_no_retry_and_no_late_reply() {
    let peer = Harness::new(Script {
        turn: Arc::new(|session, _| {
            vec![
                ev(ApiEvent::MessageAccepted {
                    session_id: session.into(),
                }),
                Step::Sleep(Duration::from_millis(100)),
                Step::Disconnect,
            ]
        }),
        ..Script::default()
    });
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    g.connect();
    g.focus_composer();
    g.type_text("届いたか不明");
    g.frame(vec![enter()]);
    let f = g.until("uncertain", Duration::from_secs(8), |f, _| {
        f.has("結果不明")
    });
    assert!(f.has("自動再送しません"), "{}", f.joined());
    no_demo(&f);
    assert_eq!(
        g.app.composer(),
        "",
        "uncertain send is not offered as a draft to resend"
    );
    let f = g.until("disconnected", Duration::from_secs(5), |f, _| {
        f.has("未接続")
    });
    assert!(
        f.has("新しいチャットで続けてください") || f.has("接続"),
        "{}",
        f.joined()
    );
    g.focus_composer();
    g.type_text("再送");
    g.frame(vec![enter()]);
    g.idle(Duration::from_millis(300));
    assert_eq!(sends(&peer).len(), 1, "resent after uncertain outcome");
    assert!(!g.roles().contains(&Role::Assistant));
}

#[test]
fn cancel_is_responsive_and_the_cancelled_draft_comes_back() {
    let peer = Harness::new(Script {
        turn: Arc::new(|session, _| {
            vec![
                ev(ApiEvent::MessageAccepted {
                    session_id: session.into(),
                }),
                Step::Sleep(Duration::from_secs(30)),
                delta(session, Some("m1"), "too late"),
                done(session, Some("m1")),
                turn_done(session),
            ]
        }),
        ..Script::default()
    });
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    g.connect();
    g.focus_composer();
    g.type_text("長い質問");
    g.frame(vec![enter()]);
    g.until("pending", Duration::from_secs(2), |f, _| {
        f.has("キャンセル")
    });
    let started = Instant::now();
    g.click("キャンセル");
    g.until("cancelled", Duration::from_secs(6), |_, app| {
        app.chat().active().is_some_and(|s| !s.is_loading())
    });
    assert!(started.elapsed() < Duration::from_secs(6));
    assert_eq!(peer.count(|r| matches!(r, ApiRequest::Cancel { .. })), 1);
    assert_eq!(
        g.app.composer(),
        "長い質問",
        "confirmed cancel keeps the draft"
    );
    let f = g.idle(Duration::from_millis(300));
    assert!(!f.has("too late"));
    no_demo(&f);
    assert_eq!(sends(&peer).len(), 1);
}

#[test]
fn ime_composition_never_sends_in_real_mode_and_shift_enter_is_a_newline() {
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    g.connect();
    g.focus_composer();
    let preedit = |s: &str| {
        Event::Ime(ImeEvent::Preedit {
            text: s.into(),
            active_range_chars: None,
        })
    };
    let commit = |s: &str| Event::Ime(ImeEvent::Commit(s.into()));
    g.frame(vec![preedit("にほんご")]);
    g.frame(vec![enter()]);
    g.frame(vec![commit("日本語"), enter()]);
    g.frame(vec![enter(), commit("を")]);
    g.frame(vec![key(Key::Enter, Modifiers::SHIFT)]);
    g.type_text("送る");
    g.idle(Duration::from_millis(200));
    assert!(
        sends(&peer).is_empty(),
        "IME/Shift+Enter sent: {:?}",
        sends(&peer)
    );
    assert_eq!(g.app.composer(), "日本語を\n送る");
    g.frame(vec![enter()]);
    g.until("reply", Duration::from_secs(5), |f, _| {
        f.has("scripted reply")
    });
    assert_eq!(
        sends(&peer),
        [("sess-0".to_owned(), "日本語を\n送る".to_owned())]
    );
}

#[test]
fn imported_snapshot_is_never_forwarded_to_the_model() {
    let goal = json!({"purpose": "PRIVATE-PURPOSE", "constraints": [], "success_conditions": [],
        "owned_paths": [], "unfinished": []});
    let snap = json!({"schema": "h2o-snapshot/1", "branches": [{
        "spec": {"run_id": "PRIVATE-RUN", "branch_id": "b", "owner_session_id": "root",
            "session_id": "w", "goal": goal, "context_ids": [], "max_corrections": 1},
        "checkpoints": [], "receipts": [], "needs_reconciliation": null}]});
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    assert!(g.app.monitor_mut().import_pasted(&snap.to_string()));
    g.connect();
    g.focus_composer();
    g.type_text("状態は？");
    g.frame(vec![enter()]);
    g.until("reply", Duration::from_secs(5), |f, _| {
        f.has("scripted reply")
    });
    let all = format!("{:?}", peer.requests());
    assert!(!all.contains("PRIVATE"), "snapshot reached the peer: {all}");
}

#[test]
fn demo_is_only_an_explicit_choice_and_never_calls_the_sdk() {
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    let before = g.frame(vec![]);
    assert!(
        !before.has("デモ・未接続"),
        "demo before an explicit choice: {}",
        before.joined()
    );
    g.click("デモモード");
    let f = g.frame(vec![]);
    assert!(f.has("デモ・未接続"), "{}", f.joined());
    g.focus_composer();
    g.type_text("デモで試す");
    g.frame(vec![enter()]);
    let f = g.idle(Duration::from_millis(800));
    assert!(f.has("デモ応答（未接続）"), "{}", f.joined());
    assert!(peer.endpoints().is_empty() && peer.requests().is_empty());
    assert_eq!(g.app.chat().active().unwrap().kind, Kind::Demo);
}

#[test]
fn connection_controls_and_errors_fit_narrow_width_and_follow_both_themes() {
    let mut script = Script::default();
    script.capabilities.retain(|c| c != "session_tools");
    let peer = Harness::new(script);
    let mut g = Gui::new(&peer, 360.0, 1200.0);
    g.click(ENDPOINT_HINT);
    g.type_text(TEST_ENDPOINT);
    g.click("接続");
    let light = g.until("failure", Duration::from_secs(5), |f, _| {
        f.has("接続エラー")
    });
    for t in &light.texts {
        assert!(
            t.rect.right() <= 360.5,
            "overflow: {:?} {:?}",
            t.text,
            t.rect
        );
    }
    assert_eq!(light.text("未接続").rows, 1, "status row wraps");
    assert!(luminance(light.bg()) > 0.8, "light default");
    for (rect, fill) in &light.fills {
        assert!(
            luminance(*fill) > 0.45,
            "dark fill in light mode: {fill:?} {rect:?}"
        );
    }
    assert_readable(&light, "light");

    let dark = g.click("ダーク");
    assert!(luminance(dark.bg()) < 0.05);
    for (rect, fill) in &dark.fills {
        assert!(
            luminance(*fill) < 0.2,
            "light fill in dark mode: {fill:?} {rect:?}"
        );
    }
    assert!(
        dark.has("接続エラー") && dark.has(TEST_ENDPOINT),
        "{}",
        dark.joined()
    );
    let (l, d) = (light.text("接続エラー"), dark.text("接続エラー"));
    assert_ne!(
        (l.color, l.backdrop),
        (d.color, d.backdrop),
        "error ignores theme"
    );
    assert_readable(&dark, "dark");
}

#[test]
fn real_reply_and_failure_bubbles_follow_both_themes_and_stay_readable() {
    let peer = Harness::new(Script {
        turn: Arc::new(|session, content| {
            if content == "失敗" {
                vec![
                    ev(ApiEvent::MessageAccepted {
                        session_id: session.into(),
                    }),
                    turn_done(session),
                ]
            } else {
                reply_steps(session, "実際の返答です")
            }
        }),
        ..Script::default()
    });
    let mut g = Gui::new(&peer, 1280.0, 1400.0);
    g.connect();
    g.focus_composer();
    g.type_text("質問");
    g.frame(vec![enter()]);
    g.until("reply", Duration::from_secs(5), |f, _| {
        f.has("実際の返答です")
    });
    g.focus_composer();
    g.type_text("失敗");
    g.frame(vec![enter()]);
    let light = g.until("notice", Duration::from_secs(5), |f, _| {
        f.has("送信できませんでした")
    });
    assert!(luminance(light.bg()) > 0.8, "light default");
    for (rect, fill) in &light.fills {
        assert!(
            luminance(*fill) > 0.45,
            "dark fill in light mode: {fill:?} {rect:?}"
        );
    }
    assert_readable(&light, "light");
    let dark = g.click("ダーク");
    for (rect, fill) in &dark.fills {
        assert!(
            luminance(*fill) < 0.2,
            "light fill in dark mode: {fill:?} {rect:?}"
        );
    }
    assert_readable(&dark, "dark");
    for needle in [
        ASSISTANT,
        "実際の返答です",
        "エラー（実接続",
        READY,
        "経路（jcodeの報告）",
    ] {
        let (l, d) = (light.text(needle), dark.text(needle));
        assert_ne!(
            (l.color, l.backdrop),
            (d.color, d.backdrop),
            "{needle:?} ignores the theme"
        );
    }
    assert_eq!(g.app.composer(), "失敗", "draft survives the theme switch");
}

#[test]
fn explicit_disconnect_then_reconnect_opens_a_fresh_isolated_session() {
    let peer = Harness::new(Script {
        turn: echo(),
        ..Script::default()
    });
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    g.connect();
    g.focus_composer();
    g.type_text("前半");
    g.frame(vec![enter()]);
    g.until("reply", Duration::from_secs(5), |f, _| {
        f.has("echo[sess-0]: 前半")
    });
    let first = g.app.chat().active_id().unwrap();
    g.click("切断");
    let f = g.until("disconnected", Duration::from_secs(5), |f, _| {
        f.has("未接続")
    });
    assert!(f.has("新しいチャットで続けてください"), "{}", f.joined());
    assert_eq!(peer.endpoints().len(), 1, "no reconnect by itself");
    g.idle(Duration::from_millis(300));
    assert_eq!(peer.endpoints().len(), 1, "no reconnect by itself");

    g.click("接続");
    g.until("ready again", Duration::from_secs(5), |f, _| f.has(READY));
    assert_ne!(
        g.app.chat().active_id(),
        Some(first),
        "old history is not continued by a new session"
    );
    g.focus_composer();
    g.type_text("後半");
    g.frame(vec![enter()]);
    g.until("reply", Duration::from_secs(5), |f, _| {
        f.has("echo[sess-1]: 後半")
    });
    assert_eq!(peer.endpoints().len(), 2);
    assert_eq!(
        sends(&peer).len(),
        2,
        "nothing resent on reconnect: {:?}",
        sends(&peer)
    );
}

/// Truthful storage/context disclosure: this UI keeps history only in memory, but what a real
/// chat sends MAY persist on the jcode daemon/runtime side, and closing the app proves no
/// deletion. Per-turn isolated sessions and the bounded replay of accepted turns are visible
/// before any prompt; "connected" is a logical ready state, not a held socket.
#[test]
fn storage_and_replay_disclosure_is_visible_before_a_real_prompt_on_every_surface() {
    const LOCAL: &str = "この画面の履歴はメモリ内のみ";
    const REMOTE: &str = "jcode側（daemon/runtime）に保存される場合があります";
    const MONITOR_REMOTE: &str = "jcode側の保存: 不明";
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    let f = g.frame(vec![]);
    for old in ["履歴は保存されません（メモリ内のみ）", "履歴: このウィンドウのメモリ内のみ"] {
        assert!(!f.has(old), "misleading memory-only claim {old:?}: {}", f.joined());
    }
    let side = f.text(LOCAL);
    assert!(side.rect.right() < 400.0, "sidebar scope: {side:?}");
    assert!(side.text.contains(REMOTE), "sidebar names daemon-side persistence: {side:?}");
    let mon = f.text(MONITOR_REMOTE);
    assert!(mon.rect.left() > 900.0, "monitor scope: {mon:?}");
    let notice = f
        .texts
        .iter()
        .find(|t| t.text.contains("32,000文字") && t.rect.left() > 250.0 && t.rect.right() < 950.0)
        .unwrap_or_else(|| panic!("chat-surface notice: {}", f.joined()));
    for needle in [REMOTE, "削除されたとは限りません", "新しい隔離セッション", "応答済み", "古い分は省略", "スナップショットは送りません"] {
        assert!(notice.text.contains(needle), "{needle:?} in {:?}", notice.text);
    }
    assert!(peer.requests().is_empty(), "disclosure needs no call");

    let ready = g.connect();
    assert!(ready.has("接続設定済み") && ready.has("送信ごとに再確認"), "{}", ready.joined());
    assert!(!ready.has("接続済み"), "no persistent-socket claim: {}", ready.joined());
    assert!(ready.has("32,000文字"), "notice stays after connect: {}", ready.joined());
    assert!(sends(&peer).is_empty());

    // 360 px narrow, both themes: the notice wraps (never only a tooltip), fits and is readable
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 360.0, 1200.0);
    for theme in ["light", "dark"] {
        if theme == "dark" {
            g.click("ダーク");
        }
        let chat = g.click("チャット");
        let n = chat.text("32,000文字");
        assert!(n.rows > 1 && n.text.contains(REMOTE), "{theme}: {n:?}");
        for t in &chat.texts {
            assert!(t.rect.right() <= 360.5, "{theme} overflow: {:?} {:?}", t.text, t.rect);
        }
        assert_readable(&chat, theme);
        let hist = g.click("履歴");
        assert!(hist.text(LOCAL).text.contains(REMOTE), "{theme}: {}", hist.joined());
        assert_readable(&hist, theme);
        let mon = g.click("監視");
        assert!(mon.has(MONITOR_REMOTE) && mon.has(LOCAL), "{theme}: {}", mon.joined());
        assert_readable(&mon, theme);
    }
    assert!(peer.requests().is_empty());
}

// ---- supported minimum window (bin/h2o-native.rs min_inner_size 360×480) ----------------------

const MIN_W: f32 = 360.0;
const MIN_H: f32 = 480.0;
const NOTICE: &str = "保存と文脈";

impl Frame {
    /// `label` (exact, else contained) painted entirely inside its clip rectangle and the window.
    fn shown(&self, label: &str, size: egui::Vec2) -> &Text {
        let t = self.text(label);
        let screen = Rect::from_min_size(pos2(0.0, 0.0), size).expand(0.5);
        assert!(
            screen.contains_rect(t.rect) && t.clip.expand(1.0).contains_rect(t.rect),
            "{label:?} not fully on screen: {:?} clip {:?}; painted: {}",
            t.rect,
            t.clip,
            self.joined()
        );
        t
    }

    fn fits_width(&self, what: &str) {
        for t in &self.texts {
            // the single-line endpoint field scrolls its own typed text inside its clip
            let r = if t.text.starts_with(TEST_ENDPOINT) { t.rect.intersect(t.clip) } else { t.rect };
            assert!(r.right() <= MIN_W + 0.5, "{what} overflow: {:?} {:?}", t.text, t.rect);
        }
    }
}

impl Gui {
    /// A real mouse-wheel scroll with the pointer at `at`, then frames for the smoothed scroll.
    fn wheel(&mut self, at: egui::Pos2, dy: f32) -> Frame {
        self.frame(vec![
            Event::PointerMoved(at),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, dy),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::NONE,
            },
        ]);
        for _ in 0..30 {
            self.frame(vec![]);
        }
        self.frame(vec![])
    }

    /// Clicks only what the user can actually see.
    fn click_shown(&mut self, label: &str) -> Frame {
        self.frame(vec![]).shown(label, self.size);
        self.click(label)
    }

    /// Scrolls the storage/context notice through its viewport with the real wheel and proves
    /// every row of it was on screen at some point, while `action` (接続/送信) never left the
    /// screen. Ends scrolled back to the top; returns where the wheel was used.
    fn read_whole_notice(&mut self, what: &str, action: &str) -> egui::Pos2 {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), self.size);
        let visible = |t: &Text| {
            let v = t.clip.intersect(screen);
            (v.top() - t.rect.top(), v.bottom() - t.rect.top())
        };
        let notice = |f: &Frame| f.texts.iter().find(|t| t.text.contains(NOTICE)).cloned();
        let mut f = self.frame(vec![]);
        // the notice may start below a long error, in the same scroll area
        let anchor = notice(&f).unwrap_or_else(|| f.text("接続エラー").clone());
        let at = anchor.clip.intersect(screen).center();
        let mut seen: Option<f32> = None;
        let mut tallest = 0.0f32;
        let mut height = f32::INFINITY;
        for _ in 0..80 {
            f.shown(action, self.size);
            if let Some(n) = notice(&f) {
                height = n.rect.height();
                let (a, b) = visible(&n);
                if b - a > 1.0 && b > 0.0 {
                    let from = seen.unwrap_or(0.0);
                    assert!(a <= from + 2.0, "{what}: notice rows {from}..{a} never on screen: {n:?}");
                    seen = Some(from.max(b));
                    tallest = tallest.max(b.min(height) - a.max(0.0));
                }
            }
            if seen.is_some_and(|s| s >= height - 1.0) {
                break;
            }
            f = self.wheel(at, -24.0);
        }
        assert!(
            seen.is_some_and(|s| s >= height - 1.0),
            "{what}: notice tail unreachable (seen {seen:?} of {height}); painted: {}",
            f.joined()
        );
        assert!(tallest >= 28.0, "{what}: notice viewport too small to read: {tallest}");
        let n = notice(&f).expect("notice");
        assert!(n.text.contains("削除されたとは限りません") && n.text.contains("32,000文字"));
        self.wheel(at, 10_000.0).shown(action, self.size);
        at
    }
}

/// Every Send echoes, except a prompt containing 待って, which waits (so it can be cancelled).
fn echo_or_wait() -> TurnScript {
    Arc::new(|session, content| {
        if content.contains("待って") {
            return vec![
                ev(ApiEvent::MessageAccepted {
                    session_id: session.into(),
                }),
                Step::Sleep(Duration::from_secs(30)),
                delta(session, Some("m1"), "too late"),
                done(session, Some("m1")),
                turn_done(session),
            ];
        }
        reply_steps(session, &format!("echo[{session}]: {content}"))
    })
}

#[test]
fn minimum_window_keeps_send_and_cancel_usable_and_the_notice_readable_in_both_themes() {
    for theme in ["light", "dark"] {
        let peer = Harness::new(Script {
            turn: echo_or_wait(),
            ..Script::default()
        });
        let mut g = Gui::new(&peer, MIN_W, MIN_H);
        if theme == "dark" {
            g.click_shown("ダーク");
        }
        let size = g.size;
        // before connect
        let f = g.frame(vec![]);
        f.shown("送信", size);
        f.shown("接続", size);
        f.shown(ENDPOINT_HINT, size);
        f.fits_width(theme);
        assert_readable(&f, theme);
        g.read_whole_notice(&format!("{theme} before connect"), "接続");
        assert!(peer.requests().is_empty(), "reading needs no call");

        // explicit connect with real clicks, then Ready
        g.click_shown(ENDPOINT_HINT);
        g.type_text(TEST_ENDPOINT);
        g.click_shown("接続");
        let f = g.until("ready", Duration::from_secs(5), |f, _| f.has(READY));
        f.shown("送信", size);
        f.shown("切断", size);
        f.fits_width(theme);
        assert_readable(&f, theme);
        g.read_whole_notice(&format!("{theme} ready"), "送信");
        assert!(sends(&peer).is_empty(), "notice is read before any prompt");

        // a pointer click on 送信 delivers the prompt and the actual SDK reply
        g.click_shown(COMPOSER_HINT);
        g.type_text("こんにちは");
        g.click_shown("送信");
        let f = g.until("reply", Duration::from_secs(5), |f, _| {
            f.has("echo[sess-0]: こんにちは")
        });
        f.shown("送信", size);
        assert_eq!(sends(&peer), [("sess-0".to_owned(), "こんにちは".to_owned())]);

        // pending: キャンセル stays on screen and works by pointer
        g.click_shown(COMPOSER_HINT);
        g.type_text("待ってください");
        g.click_shown("送信");
        let f = g.until("pending", Duration::from_secs(5), |f, _| f.has("キャンセル"));
        f.fits_width(theme);
        g.click_shown("キャンセル");
        let f = g.until("cancelled", Duration::from_secs(6), |_, app| {
            app.chat().active().is_some_and(|s| !s.is_loading())
        });
        assert_eq!(peer.count(|r| matches!(r, ApiRequest::Cancel { .. })), 1, "{theme}");
        assert_eq!(g.app.composer(), "待ってください", "{theme}: draft back");
        f.shown("送信", size);
        f.fits_width(theme);
        assert!(!g.idle(Duration::from_millis(200)).has("too late"));
        assert_eq!(sends(&peer).len(), 2, "{theme}: no resend");
    }
}

#[test]
fn minimum_window_long_error_and_route_never_push_the_composer_away() {
    for theme in ["light", "dark"] {
        // a long connection error (the handshake error names the typed endpoint)
        let peer = Harness::new(Script {
            hello: Reply::Error,
            ..Script::default()
        });
        let mut g = Gui::new(&peer, MIN_W, MIN_H);
        if theme == "dark" {
            g.click_shown("ダーク");
        }
        let size = g.size;
        let long_endpoint = format!("{TEST_ENDPOINT}/{}", "とても長いパス名".repeat(30));
        g.click_shown(ENDPOINT_HINT);
        g.type_text(&long_endpoint);
        g.click_shown("接続");
        let f = g.until("failure", Duration::from_secs(5), |f, _| f.has("接続エラー"));
        let err = f.text("接続エラー").clone();
        assert!(err.rows > 3, "{theme}: error is long: {err:?}");
        assert!(
            err.clip.contains(err.rect.left_top() + vec2(1.0, 14.0)),
            "{theme}: error start must be on screen: {err:?}"
        );
        f.shown("接続", size);
        f.shown("送信", size);
        f.shown(COMPOSER_HINT, size);
        f.fits_width(theme);
        assert_readable(&f, theme);
        g.read_whole_notice(&format!("{theme} error"), "接続");
        g.click_shown(COMPOSER_HINT);
        g.type_text("下書き");
        assert_eq!(g.app.composer(), "下書き");
        assert!(sends(&peer).is_empty());

        // a long reported route
        let peer = Harness::new(Script {
            provider: Some("provider-".repeat(20)),
            model: Some("長いモデル名".repeat(30)),
            ..Script::default()
        });
        let mut g = Gui::new(&peer, MIN_W, MIN_H);
        if theme == "dark" {
            g.click_shown("ダーク");
        }
        g.click_shown(ENDPOINT_HINT);
        g.type_text(TEST_ENDPOINT);
        g.click_shown("接続");
        let f = g.until("ready", Duration::from_secs(5), |f, _| f.has(READY));
        f.shown("送信", size);
        f.shown("切断", size);
        f.fits_width(theme);
        assert_readable(&f, theme);
        let at = g.read_whole_notice(&format!("{theme} long route"), "送信");
        // the route (one truncated row) is below the notice, reachable in the same scroll area
        let mut f = g.wheel(at, -10_000.0);
        for _ in 0..10 {
            let r = f.text("経路（jcodeの報告）: provider-");
            if r.clip.contains_rect(r.rect.shrink(1.0)) {
                break;
            }
            f = g.wheel(at, 6.0);
        }
        let route = f.text("経路（jcodeの報告）: provider-");
        assert!(route.clip.contains_rect(route.rect.shrink(1.0)), "{theme}: {route:?}");
        assert!(f.has("初回の実応答: 未確認"), "{}", f.joined());
        f.shown("送信", size);
        f.fits_width(theme);
        assert!(sends(&peer).is_empty());
    }
}

/// The replay rule as chat_transport::replay implements it: the most recent CONTIGUOUS accepted
/// whole turns whose q+a BODY chars total ≤ 32,000, rendered chronologically; labels, separators
/// and the omission note come on top; an oversize newest turn drops all earlier context. It is
/// not a cap on the whole payload, the current prompt, the UI history or tokens.
#[test]
fn replay_disclosure_states_the_body_only_contiguous_chronological_cap() {
    let peer = Harness::new(Script::default());
    let mut g = Gui::new(&peer, 1280.0, 900.0);
    let n = g.frame(vec![]).text(NOTICE).text.clone();
    assert!(!n.contains("新しい順に最大32,000文字"), "imprecise order/budget: {n}");
    for needle in [
        "直近から連続する",
        "往復単位",
        "質問と応答の本文合計32,000文字以内",
        "時系列順",
        "見出し・区切り・省略の説明は別",
        "直近の1往復だけで超える場合はそれ以前の文脈を全て省略",
        "今回の入力",
        "上限ではありません",
        "失敗・キャンセルの分は送りません",
        "jcode側（daemon/runtime）に保存される場合があります",
        "削除されたとは限りません",
    ] {
        assert!(n.contains(needle), "{needle:?} missing in {n:?}");
    }
    assert!(peer.requests().is_empty());
}

/// From evidence/review-r2-final-ui-probes.rs (final round-2 minimum-height reproducer). Only
/// change: the notice now sits in a bounded scroll area, so its painted (clipped) part is measured
/// instead of the whole galley; reading all of it is proven by `read_whole_notice` above.
#[test]
fn final_minimum_height_notice_and_composer_are_visible() {
 let mut fits=true;
 for theme in ["light","dark"] {
  let peer=Harness::new(Script::default()); let mut g=Gui::new(&peer,360.0,480.0);
  if theme=="dark" {g.click("ダーク");}
  let f=g.frame(vec![]);
  let n=f.text("32,000文字"); let s=f.text("送信");
  eprintln!("{theme}: notice={:?}, send={:?}",n.rect,s.rect);
  fits &= n.rect.intersect(n.clip).bottom() <=480.5 && s.rect.bottom()<=480.5;
  let f=g.connect(); let n=f.text("32,000文字"); let s=f.texts.iter().find(|t|t.text=="送信");
  eprintln!("{theme} READY: notice={:?}, exact send={:?}",n.rect,s.map(|s|s.rect));
  fits &= n.rect.intersect(n.clip).bottom()<=480.5 && s.is_some_and(|s|s.rect.bottom()<=480.5);
 }
 assert!(fits,"notice/send extend below minimum viewport");
}
