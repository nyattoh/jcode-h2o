#![cfg(feature = "embedded-runtime")]
//! Integration tests for the embedded runtime lifecycle and security invariants.

use h2o_drift_prototype::{
    chat_transport::{ChatController, ChatEvent, ConnectConfig},
    embedded_runtime::EmbeddedRuntime,
    native::{Bridge, Kind},
    native_ui::{FontStatus, NativeApp},
};
use std::time::{Duration, Instant};

#[test]
fn embedded_runtime_starts_and_cleans_up_on_drop() {
    let runtime = EmbeddedRuntime::start().expect("embedded runtime start");
    let api_socket = runtime.api_socket().to_path_buf();
    let temp_dir = runtime.temp_dir().to_path_buf();

    assert!(temp_dir.exists(), "temp dir must exist while running");
    assert!(
        jcode_app_core::transport::is_socket_path(&api_socket),
        "API socket listener must be active"
    );

    // Verify Drop cleans up tasks and temp directory
    drop(runtime);

    // Give OS a short moment to finalize deletion
    let deadline = Instant::now() + Duration::from_secs(3);
    while temp_dir.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !temp_dir.exists(),
        "temp dir should be cleaned up after runtime Drop"
    );
}

#[test]
fn embedded_runtime_controller_handshake_and_tool_exclusion() {
    let runtime = EmbeddedRuntime::start().expect("embedded runtime start");
    let api_socket = runtime.api_socket().to_path_buf();

    // Connect using real ChatController
    let mut controller = ChatController::new();
    let config = ConnectConfig {
        endpoint: api_socket,
        request_timeout: Duration::from_secs(5),
        turn_timeout: Duration::from_secs(10),
    };
    controller.connect(config, 42).expect("controller connect");

    // Wait for ChatEvent::Ready (validating session_tools, NeutralDir, system prompt override, tool exclusion ACK)
    let start = Instant::now();
    let mut ready_session = None;
    while start.elapsed() < Duration::from_secs(10) {
        if let Some(event) = controller.try_recv() {
            match event {
                ChatEvent::Ready {
                    conversation_id,
                    session_id,
                    ..
                } => {
                    assert_eq!(conversation_id, 42);
                    ready_session = Some(session_id);
                    break;
                }
                ChatEvent::Failed { message, .. } => {
                    panic!("controller connect failed: {message}");
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        ready_session.is_some(),
        "embedded runtime did not produce Ready within timeout"
    );

    // Clean disconnect
    controller.disconnect();
    drop(runtime);
}

#[test]
fn native_app_embedded_runtime_state_machine() {
    let mut app = NativeApp::new(FontStatus::Missing);

    // Invariant 1: Zero side-effects on startup!
    assert!(
        !app.is_embedded_runtime_running(),
        "zero side effects on launch"
    );
    assert_eq!(*app.chat().bridge(), Bridge::Disconnected { reason: None });

    // Invariant 2: Demo mode performs zero network/daemon connections
    app.set_mode(Kind::Demo);
    assert_eq!(app.chat().mode(), Kind::Demo);
    assert!(!app.is_embedded_runtime_running());
    let sent = app.chat_mut().send("こんにちは", 1000.0);
    assert!(sent);
    assert!(app.chat_mut().tick(1001.0));
    assert!(!app.is_embedded_runtime_running());

    // Switch back to Real mode
    app.set_mode(Kind::Real);
    assert_eq!(app.chat().mode(), Kind::Real);
    assert!(!app.is_embedded_runtime_running());

    // Invariant 3: External socket path fallback does NOT launch embedded runtime
    app.set_endpoint_input("nonexistent-test-socket.sock");
    app.connect();
    assert!(
        !app.is_embedded_runtime_running(),
        "external socket must not start embedded runtime"
    );
    app.disconnect();

    // Invariant 4: Blank endpoint triggers embedded runtime on-demand!
    app.set_endpoint_input("");
    app.connect();
    assert!(
        app.is_embedded_runtime_running(),
        "empty endpoint must launch embedded runtime"
    );

    // Invariant 5: Disconnect stops embedded runtime
    app.disconnect();
    assert!(
        !app.is_embedded_runtime_running(),
        "disconnect must stop embedded runtime"
    );
}
