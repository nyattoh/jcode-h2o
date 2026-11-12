#![cfg(feature = "native-ui")]
//! Native (egui) reception UI: pure session/demo/composer/layout/snapshot state, plus headless
//! frames through the real egui widget pipeline (TextEdit, IME events, layout). No window, no
//! daemon, no network, no keys. A real window is checked separately (evidence/N1.md).

use eframe::egui::{self, Event, ImeEvent, Key, Modifiers, Rect, Shape, pos2, vec2};
use h2o_drift_prototype::native::{
    Chat, DEMO_DELAY_SECS, ImeGuard, KeyInput, Kind, Layout, Monitor, ReceiptState, Role, Tri,
    demo_reply, layout_for, one_line, snapshot_view,
};
use h2o_drift_prototype::native_ui::{FontStatus, NativeApp, install_japanese_font};
use serde_json::{Value, json};
use std::fs;

mod support;
use support::TestDir;

// ---- fixtures --------------------------------------------------------------------------------

fn goal(purpose: &str) -> Value {
    json!({"purpose": purpose, "constraints": ["読み取り専用"], "success_conditions": ["不明が保たれる"],
           "owned_paths": ["experiments/h2o-drift"], "unfinished": []})
}

fn checkpoint(sequence: u64, signal: &str) -> Value {
    json!({
        "sequence": sequence,
        "input": {
            "goal": goal("g"),
            "observation": {"call_id": format!("call-{sequence}"), "tool": "bash",
                            "output": "PRIVATE-OUTPUT-SHOULD-NOT-RENDER",
                            "error": "PRIVATE-ERROR-SHOULD-NOT-RENDER"},
            "context": [{"id": "src", "bytes": [80, 82, 73, 86]}]
        },
        "assessment": {"signal": signal, "reason": "理由<b>literal</b>"},
        "decision": "Hold",
        "action": "Hold"
    })
}

fn sample(purpose: &str, run_id: &str) -> Value {
    json!({
        "schema": "h2o-snapshot/1",
        "captured_at": "2026-11-12T00:00:00Z",
        "branches": [{
            "spec": {"run_id": run_id, "branch_id": "branch-a", "owner_session_id": "root",
                     "session_id": "worker-a", "goal": goal(purpose), "context_ids": ["src"],
                     "max_corrections": 1},
            "checkpoints": [checkpoint(1, "Unknown"), checkpoint(2, "Aligned"), checkpoint(3, "Misaligned")],
            "receipts": [{"sequence": 1, "receipt": null}, {"sequence": 2, "receipt": {"acknowledged": true}}],
            "needs_reconciliation": null
        }]
    })
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

// ---- headless egui harness -------------------------------------------------------------------

struct Harness {
    ctx: egui::Context,
    app: NativeApp,
    time: f64,
    size: egui::Vec2,
}

struct Painted {
    text: String,
    rect: Rect,
    rows: usize,
}

impl Harness {
    fn new(width: f32, height: f32) -> Self {
        let ctx = egui::Context::default();
        let font = install_japanese_font(&ctx);
        let mut app = NativeApp::new(font);
        app.set_mode(Kind::Demo); // these tests cover the explicitly chosen demo
        let mut h = Self {
            ctx,
            app,
            time: 0.0,
            size: vec2(width, height),
        };
        h.frame(vec![]);
        h.frame(vec![]); // focus requests land on the following pass
        h
    }

    fn frame(&mut self, events: Vec<Event>) -> Vec<Painted> {
        self.time += 0.016;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear(); // no renderer in headless tests
        let mut painted = Vec::new();
        fn walk(shape: &Shape, out: &mut Vec<Painted>) {
            match shape {
                Shape::Text(t) => out.push(Painted {
                    text: t.galley.text().to_owned(),
                    rect: t.visual_bounding_rect(),
                    rows: t.galley.rows.len(),
                }),
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    fn wait(&mut self, secs: f64) -> Vec<Painted> {
        self.time += secs;
        self.frame(vec![])
    }
}

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

fn joined(p: &[Painted]) -> String {
    p.iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- chat state ------------------------------------------------------------------------------

#[test]
fn chat_starts_empty_and_send_creates_session_with_delayed_demo_reply() {
    let mut chat = Chat::default();
    chat.set_mode(Kind::Demo); // demo is an explicit choice; real chat is the default
    assert!(
        chat.sessions().is_empty(),
        "empty state must have no sessions"
    );
    assert!(chat.active().is_none());
    assert!(!chat.send("   \n ", 0.0), "blank input must not send");
    assert!(chat.sessions().is_empty());

    assert!(chat.send("こんにちは", 1.0));
    let s = chat.active().expect("send creates an active session");
    assert_eq!(s.messages.len(), 1);
    assert_eq!(s.messages[0].role, Role::User);
    assert_eq!(s.messages[0].text, "こんにちは");
    assert_eq!(s.title, "こんにちは");
    assert!(s.is_loading(), "reply is pending (loading state)");
    assert!(
        !chat.send("二通目", 1.1),
        "cannot send while the demo reply is pending"
    );

    chat.tick(1.0 + DEMO_DELAY_SECS / 2.0);
    assert!(
        chat.active().unwrap().is_loading(),
        "not resolved before the delay"
    );
    chat.tick(1.0 + DEMO_DELAY_SECS);
    let s = chat.active().unwrap();
    assert!(!s.is_loading());
    assert_eq!(s.messages.len(), 2);
    assert_eq!(s.messages[1], demo_reply("こんにちは"));
}

#[test]
fn demo_reply_is_deterministic_labelled_and_never_claims_live_answer() {
    let a = demo_reply("テスト\n二行目");
    assert_eq!(a, demo_reply("テスト\n二行目"));
    assert_eq!(a.role, Role::Demo);
    assert!(a.text.contains("デモ応答"), "{}", a.text);
    assert!(a.text.contains("未接続"), "{}", a.text);
    assert!(a.text.contains("編集や実行は行いません"), "{}", a.text);
    assert_ne!(a, demo_reply("別の入力"));
    let e = demo_reply("/demo-error");
    assert_eq!(e.role, Role::DemoError);
    assert!(e.text.contains("擬似エラー"), "{}", e.text);
}

#[test]
fn new_chat_reuses_empty_session_and_select_switches_history() {
    let mut chat = Chat::default();
    chat.set_mode(Kind::Demo); // demo is an explicit choice; real chat is the default
    let first = chat.new_session();
    assert_eq!(
        chat.new_session(),
        first,
        "an empty active session is reused"
    );
    assert_eq!(chat.sessions().len(), 1);
    assert_eq!(chat.active().unwrap().title, "新しいチャット");
    chat.send("一つ目の会話", 0.0);
    chat.tick(10.0);
    let second = chat.new_session();
    assert_ne!(second, first);
    assert_eq!(chat.active_id(), Some(second));
    chat.send("二つ目", 11.0);
    // pending reply on the second session still resolves after switching away
    assert!(chat.select(first));
    assert_eq!(chat.active().unwrap().messages[0].text, "一つ目の会話");
    chat.tick(20.0);
    assert!(chat.select(second));
    assert_eq!(chat.active().unwrap().messages.len(), 2);
    assert!(!chat.select(9999), "unknown id is rejected");
    assert_eq!(chat.active_id(), Some(second));
}

// ---- composer key decision -------------------------------------------------------------------

#[test]
fn ime_guard_blocks_enter_during_and_at_commit_of_composition() {
    let mut g = ImeGuard::default();
    assert!(
        g.frame(&[KeyInput::Enter { shift: false }]),
        "plain Enter sends"
    );
    assert!(
        !g.frame(&[KeyInput::Enter { shift: true }]),
        "Shift+Enter is a newline"
    );
    assert!(!g.frame(&[KeyInput::Preedit("にほんご".into())]));
    assert!(g.is_composing());
    assert!(
        !g.frame(&[KeyInput::Enter { shift: false }]),
        "Enter while composing must not send"
    );
    assert!(
        !g.frame(&[
            KeyInput::Commit("日本語".into()),
            KeyInput::Enter { shift: false }
        ]),
        "the Enter that commits a composition must not send"
    );
    assert!(!g.is_composing());
    assert!(
        g.frame(&[KeyInput::Enter { shift: false }]),
        "next plain Enter sends"
    );
    assert!(!g.frame(&[
        KeyInput::Preedit("か".into()),
        KeyInput::Preedit(String::new()),
        KeyInput::Enter { shift: false }
    ]));
    assert!(!g.is_composing(), "empty preedit (cancel) ends composition");
}

#[test]
fn ime_guard_any_composition_event_suppresses_send_for_the_whole_frame() {
    let enter = || KeyInput::Enter { shift: false };
    let commit = || KeyInput::Commit("日本語".into());
    let preedit = |s: &str| KeyInput::Preedit(s.into());

    let mut g = ImeGuard::default();
    assert!(!g.frame(&[enter(), commit()]), "Enter then Commit sent");
    assert!(!g.is_composing());
    assert!(g.frame(&[enter()]), "next genuine Enter sends");

    assert!(
        !g.frame(&[enter(), preedit("に")]),
        "Enter then Preedit sent"
    );
    assert!(g.is_composing(), "final state follows the last event");
    assert!(!g.frame(&[commit(), enter()]), "Commit then Enter sent");
    assert!(!g.is_composing());

    // preedit-empty / commit boundary: Enter on either side of the composition end
    assert!(!g.frame(&[preedit("か")]));
    assert!(
        !g.frame(&[enter(), preedit("")]),
        "Enter then empty Preedit sent"
    );
    assert!(!g.is_composing());
    assert!(
        !g.frame(&[preedit(""), enter()]),
        "empty Preedit then Enter sent"
    );
    assert!(!g.frame(&[preedit("か")]));
    assert!(!g.frame(&[preedit(""), commit(), enter()]));
    assert!(!g.frame(&[enter(), preedit(""), commit()]));
    assert!(!g.is_composing());
    assert!(
        g.frame(&[enter()]),
        "genuine Enter after the boundary sends"
    );

    assert!(
        !g.frame(&[KeyInput::Enter { shift: true }]),
        "Shift+Enter is a newline"
    );
    assert!(!g.frame(&[KeyInput::Enter { shift: true }, commit()]));
    assert!(g.frame(&[enter()]));
}

// ---- layout / text ---------------------------------------------------------------------------

#[test]
fn layout_breakpoints() {
    assert_eq!(layout_for(360.0), Layout::Narrow);
    assert_eq!(layout_for(759.0), Layout::Narrow);
    assert_eq!(layout_for(760.0), Layout::Medium);
    assert_eq!(layout_for(1179.0), Layout::Medium);
    assert_eq!(layout_for(1180.0), Layout::Wide);
}

#[test]
fn one_line_flattens_controls_and_bidi_and_truncates_by_chars() {
    assert_eq!(one_line("a\nb\tc\r\u{7}", 20), "a b c �");
    assert_eq!(one_line("abc\u{202E}def\u{2066}", 20), "abc�def�");
    assert_eq!(one_line("日本語のながい文字列", 5), "日本語のな…");
    assert_eq!(one_line("短い", 5), "短い");
}

// ---- snapshot --------------------------------------------------------------------------------

#[test]
fn snapshot_view_preserves_unknowns_and_drops_private_payloads() {
    let view = snapshot_view(&bytes(&sample("目的", "run-1")), "貼り付け").expect("valid sample");
    assert_eq!(view.source, "貼り付け");
    assert_eq!(view.captured_at.as_deref(), Some("2026-11-12T00:00:00Z"));
    let b = &view.branches[0];
    assert_eq!(b.run_id, "run-1");
    assert_eq!(b.purpose, "目的");
    assert_eq!(b.needs_reconciliation, Tri::Unknown);
    let receipts: Vec<_> = b.checkpoints.iter().map(|c| c.receipt).collect();
    assert_eq!(
        receipts,
        [
            ReceiptState::Null,
            ReceiptState::Acknowledged,
            ReceiptState::Missing
        ]
    );
    assert_eq!(b.checkpoints[0].signal, "不明");
    assert_eq!(b.checkpoints[1].signal, "整合");
    assert_eq!(b.checkpoints[2].signal, "逸脱");
    assert_eq!(
        b.checkpoints[0].reason, "理由<b>literal</b>",
        "strings kept literal"
    );
    let dump = format!("{view:?}");
    assert!(
        !dump.contains("PRIVATE"),
        "observation output/error must not be kept: {dump}"
    );
    assert!(!dump.contains("80, 82"), "context bytes must not be kept");
    let mut absent = sample("x", "r");
    absent["branches"][0]["needs_reconciliation"] = json!(true);
    absent.as_object_mut().unwrap().remove("captured_at");
    let v = snapshot_view(&bytes(&absent), "f").unwrap();
    assert_eq!(v.captured_at, None);
    assert_eq!(v.branches[0].needs_reconciliation, Tri::Yes);
}

#[test]
fn invalid_import_keeps_previous_snapshot_and_error_hides_values() {
    let mut m = Monitor::default();
    assert!(m.import_pasted(&serde_json::to_string(&sample("最初", "run-1")).unwrap()));
    assert!(m.last_error.is_none());

    let mut bad = sample("二番目", "run-2");
    bad["branches"][0]["checkpoints"][0]["input"]["observation"]["output"] = json!(12345);
    bad["branches"][0]["checkpoints"][0]["input"]["observation"]["error"] = json!("SECRET-ERR");
    assert!(!m.import_pasted(&serde_json::to_string(&bad).unwrap()));
    let err = m.last_error.clone().expect("error shown");
    assert!(
        !err.contains("12345") && !err.contains("SECRET"),
        "error leaks a value: {err}"
    );
    assert_eq!(
        m.snapshot.as_ref().unwrap().branches[0].run_id,
        "run-1",
        "prior state kept"
    );

    let mut unknown = sample("x", "r");
    unknown["extra"] = json!(1);
    assert!(!m.import_pasted(&serde_json::to_string(&unknown).unwrap()));
    assert!(!m.import_pasted("not json"));
    assert!(!m.import_pasted(&"x".repeat(1024 * 1024 + 1)));
    assert!(m.last_error.as_deref().unwrap().contains("1 MiB"));
    assert_eq!(m.snapshot.as_ref().unwrap().branches[0].run_id, "run-1");

    let dir = TestDir::new();
    let path = dir.path.join("snap.json");
    fs::write(&path, bytes(&sample("ファイル", "run-file"))).unwrap();
    assert!(m.import_file(&path));
    assert!(m.last_error.is_none());
    assert_eq!(m.snapshot.as_ref().unwrap().branches[0].run_id, "run-file");
    assert!(m.snapshot.as_ref().unwrap().source.contains("snap.json"));
    assert!(!m.import_file(&dir.path.join("missing.json")));
    assert_eq!(m.snapshot.as_ref().unwrap().branches[0].run_id, "run-file");
}

#[test]
fn import_errors_never_echo_private_keys_or_values() {
    // Synthetic markers. Each input fails the gate; none of its key or value text may be shown.
    let mut envelope = sample("x", "r");
    envelope["PRIVATE_ENVELOPE_KEY_9f2"] = json!("PRIVATE_ENVELOPE_VALUE");
    let mut branch = sample("x", "r");
    branch["branches"][0]["PRIVATE_BRANCH_KEY_9f2"] = json!("PRIVATE_BRANCH_VALUE");
    let mut receipt = sample("x", "r");
    receipt["branches"][0]["receipts"][0]["PRIVATE_RECEIPT_KEY_9f2"] =
        json!("PRIVATE_RECEIPT_VALUE");
    // a quoted value that itself contains the " at line " position marker
    let mut quoted = sample("x", "r");
    quoted["branches"] = json!("x at line PRIVATE_QUOTED_VALUE");
    let mut variant = sample("x", "r");
    variant["branches"][0]["checkpoints"][0]["decision"] = json!("x at line PRIVATE_VARIANT");
    let cases = [
        ("envelope", envelope),
        ("branch", branch),
        ("receipt", receipt),
        ("quoted", quoted),
        ("variant", variant),
    ];

    let dir = TestDir::new();
    let mut m = Monitor::default();
    assert!(m.import_pasted(&serde_json::to_string(&sample("最初", "run-1")).unwrap()));
    for (name, doc) in &cases {
        let text = serde_json::to_string(doc).unwrap();
        let path = dir.path.join(format!("{name}.json"));
        fs::write(&path, &text).unwrap();
        for (how, ok) in [
            ("paste", m.import_pasted(&text)),
            ("file", m.import_file(&path)),
        ] {
            assert!(!ok, "{name}/{how} accepted");
            let err = m.last_error.clone().expect("error shown");
            assert!(!err.is_empty());
            assert!(!err.contains("PRIVATE"), "{name}/{how} echoed input: {err}");
            assert_eq!(
                m.snapshot.as_ref().unwrap().branches[0].run_id,
                "run-1",
                "{name}/{how}: prior state kept"
            );
        }
    }

    // trusted parser positions are still shown for plain syntax errors
    assert!(!m.import_pasted("{\n  \"schema\": oops"));
    let err = m.last_error.clone().unwrap();
    assert!(err.contains("line 2"), "syntax position lost: {err}");

    // and the error pane itself paints none of it
    let mut h = Harness::new(1280.0, 800.0);
    let path = dir.path.join("envelope.json");
    assert!(!h.app.import_snapshot_file(&path));
    h.app.show_monitor_pane();
    let all = joined(&h.frame(vec![]));
    assert!(all.contains("読み込めませんでした"), "{all}");
    assert!(!all.contains("PRIVATE"), "error pane echoed input: {all}");
}

// ---- real egui frames ------------------------------------------------------------------------

#[test]
fn japanese_font_is_loaded_when_a_known_system_font_exists() {
    let ctx = egui::Context::default();
    let status = install_japanese_font(&ctx);
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    let has = ctx.fonts_mut(|f| f.has_glyphs(&egui::FontId::proportional(14.0), "日本語の受付"));
    if cfg!(windows) && std::path::Path::new(r"C:\Windows\Fonts\YuGothM.ttc").exists() {
        assert!(matches!(status, FontStatus::Loaded { .. }), "{status:?}");
    }
    assert_eq!(
        has,
        matches!(status, FontStatus::Loaded { .. }),
        "{status:?}"
    );
}

#[test]
fn gui_enter_sends_shift_enter_newlines_and_shows_loading_then_demo_reply() {
    let mut h = Harness::new(1280.0, 800.0);
    let welcome = joined(&h.frame(vec![]));
    assert!(
        welcome.contains("デモ") && welcome.contains("未接続"),
        "banner: {welcome}"
    );
    assert!(welcome.contains("作業対象: 未確認"), "{welcome}");
    assert!(
        welcome.contains("履歴はまだありません"),
        "empty history state: {welcome}"
    );

    h.frame(vec![Event::Text("一行目".into())]);
    h.frame(vec![key(Key::Enter, Modifiers::SHIFT)]);
    h.frame(vec![Event::Text("二行目".into())]);
    assert_eq!(
        h.app.composer(),
        "一行目\n二行目",
        "Shift+Enter inserts a newline"
    );
    assert!(h.app.chat().sessions().is_empty(), "nothing sent yet");

    let loading = joined(&h.frame(vec![enter()]));
    assert_eq!(h.app.composer(), "", "composer cleared after send");
    let s = h.app.chat().active().unwrap();
    assert_eq!(s.messages[0].text, "一行目\n二行目");
    assert!(
        loading.contains("生成中"),
        "loading state visible: {loading}"
    );

    let done = joined(&h.wait(DEMO_DELAY_SECS + 0.1));
    assert!(done.contains("デモ応答"), "{done}");
    assert!(!h.app.chat().active().unwrap().is_loading());
}

#[test]
fn gui_ime_commit_enter_does_not_send() {
    let mut h = Harness::new(1280.0, 800.0);
    h.frame(vec![Event::Ime(ImeEvent::Preedit {
        text: "にほんご".into(),
        active_range_chars: None,
    })]);
    h.frame(vec![enter()]); // a stray Enter during composition (non-Windows integrations)
    assert!(
        h.app.chat().sessions().is_empty(),
        "sent during composition"
    );
    h.frame(vec![Event::Ime(ImeEvent::Commit("日本語".into())), enter()]);
    assert!(
        h.app.chat().sessions().is_empty(),
        "commit Enter sent the message"
    );
    assert_eq!(h.app.composer(), "日本語");
    h.frame(vec![enter()]);
    assert_eq!(h.app.chat().active().unwrap().messages[0].text, "日本語");
}

#[test]
fn gui_enter_before_ime_commit_in_same_frame_does_not_send() {
    let mut h = Harness::new(1280.0, 800.0);
    let preedit = |s: &str| {
        Event::Ime(ImeEvent::Preedit {
            text: s.into(),
            active_range_chars: None,
        })
    };
    let commit = |s: &str| Event::Ime(ImeEvent::Commit(s.into()));
    // the guard is idle (not composing) when the frame starts: a commit-only frame
    h.frame(vec![enter(), commit("日本語")]);
    assert!(
        h.app.chat().sessions().is_empty(),
        "Enter before Commit sent"
    );
    assert_eq!(h.app.composer(), "日本語");
    // preedit-empty boundary: the previous frame ended the composition
    h.frame(vec![preedit("か")]);
    h.frame(vec![preedit("")]);
    h.frame(vec![enter(), commit("を")]);
    assert!(
        h.app.chat().sessions().is_empty(),
        "Enter before Commit after cancel sent"
    );
    assert_eq!(h.app.composer(), "日本語を");
    h.frame(vec![key(Key::Enter, Modifiers::SHIFT)]);
    assert!(h.app.chat().sessions().is_empty(), "Shift+Enter sent");
    assert_eq!(h.app.composer(), "日本語を\n");
    h.frame(vec![enter()]);
    let sent = &h.app.chat().active().expect("genuine Enter sends").messages[0].text;
    assert_eq!(sent, "日本語を");
}

#[test]
fn gui_narrow_layout_keeps_rows_inside_window_and_wraps_prose() {
    let mut h = Harness::new(360.0, 1600.0); // tall so every row is painted; width is under test
    assert_eq!(h.app.layout(), Layout::Narrow);
    let long_id = format!("run-{}", "x".repeat(300));
    let purpose = "とても長い目的の説明文です。".repeat(12);
    assert!(
        h.app
            .monitor_mut()
            .import_pasted(&serde_json::to_string(&sample(&purpose, &long_id)).unwrap())
    );
    h.app.show_monitor_pane();
    let painted = h.frame(vec![]);
    let all = joined(&painted);
    assert!(all.contains("取込"), "imported provenance label: {all}");
    assert!(all.contains("読み取り専用"), "{all}");
    assert!(all.contains("不明"), "{all}");
    assert!(!all.contains("PRIVATE"), "private payload rendered");
    for p in painted.iter().filter(|p| !p.text.is_empty()) {
        assert!(
            p.rect.right() <= 360.5,
            "text overflows window: {:?} {:?}",
            p.text,
            p.rect
        );
    }
    let id_row = painted
        .iter()
        .find(|p| p.text.contains("run-xxx"))
        .expect("run id row");
    assert_eq!(id_row.rows, 1, "status row must not wrap");
    assert!(
        id_row.text.ends_with('…'),
        "status row truncated: {}",
        id_row.text
    );
    let prose = painted
        .iter()
        .find(|p| p.text.contains("とても長い目的"))
        .expect("purpose");
    assert!(prose.rows > 1, "prose should wrap");

    // a bad import keeps the previous snapshot visible and shows an error
    assert!(!h.app.monitor_mut().import_pasted("{}"));
    let after = joined(&h.frame(vec![]));
    assert!(after.contains("読み込めませんでした"), "{after}");
    assert!(
        after.contains("とても長い目的"),
        "prior snapshot still shown"
    );
}

#[test]
fn gui_session_selection_in_sidebar_switches_conversation() {
    let mut h = Harness::new(1280.0, 800.0);
    h.frame(vec![Event::Text("最初の話題".into())]);
    h.frame(vec![enter()]);
    h.wait(1.0);
    h.app.new_chat();
    h.frame(vec![]);
    h.frame(vec![Event::Text("次の話題".into())]);
    h.frame(vec![enter()]);
    let shown = joined(&h.wait(1.0));
    assert!(
        shown.contains("最初の話題") && shown.contains("次の話題"),
        "both in history: {shown}"
    );
    assert!(
        // was "保存されません": local memory-only scope, now with the daemon-side caveat
        shown.contains("この画面の履歴はメモリ内のみ") && shown.contains("保存される場合があります"),
        "local scope and daemon-side caveat stated: {shown}"
    );
    let first = h.app.chat().sessions()[0].id;
    assert!(h.app.select_session(first));
    let chat = joined(&h.frame(vec![]));
    assert_eq!(h.app.chat().active_id(), Some(first));
    assert!(chat.contains("最初の話題"));
}
