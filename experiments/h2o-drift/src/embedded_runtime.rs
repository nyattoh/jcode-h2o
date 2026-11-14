//! Embedded in-process runtime for `h2o-native`.
//!
//! Hosts the non-UI jcode daemon server (`jcode-app-core`) and Harness API bridge
//! (`jcode-harness-api-server`) entirely in-process on process-unique ephemeral
//! Windows Named Pipes (NPFS) or Unix domain sockets.
//!
//! Operates on a strict lazy-initialization model (zero side-effects on app launch)
//! and provides full RAII cleanup on disconnect or window close.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

/// RAII manager for the in-process daemon server and Harness API bridge.
pub struct EmbeddedRuntime {
    _runtime: tokio::runtime::Runtime,
    temp_dir: tempfile::TempDir,
    api_socket: PathBuf,
    server_handle: JoinHandle<()>,
    bridge_handle: JoinHandle<()>,
}

impl std::fmt::Debug for EmbeddedRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedRuntime")
            .field("api_socket", &self.api_socket)
            .field("temp_dir", &self.temp_dir.path())
            .finish()
    }
}

impl EmbeddedRuntime {
    /// Filesystem socket path (mapped to a unique named pipe on Windows) for the Harness API.
    pub fn api_socket(&self) -> &Path {
        &self.api_socket
    }

    /// Root directory of the ephemeral per-session sockets.
    pub fn temp_dir(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Starts an isolated in-process jcode daemon and Harness API bridge on-demand.
    pub fn start() -> Result<Self, String> {
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("h2o-embedded-{}-", std::process::id()))
            .tempdir()
            .map_err(|e| {
                format!("Failed to create temporary directory for embedded runtime: {e}")
            })?;

        let legacy_socket = temp_dir.path().join("h2o-legacy.sock");
        let debug_socket = temp_dir.path().join("h2o-debug.sock");
        let api_socket = temp_dir.path().join("h2o-api.sock");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("h2o-embedded-worker")
            .build()
            .map_err(|e| format!("Failed to start Tokio runtime for embedded server: {e}"))?;

        let provider = Arc::new(jcode_app_core::provider::MultiProvider::new_fast());
        let server = jcode_app_core::server::Server::new_with_paths(
            provider,
            legacy_socket.clone(),
            debug_socket,
        );

        let server_handle = runtime.spawn(async move {
            if let Err(e) = server.run().await {
                eprintln!("[h2o-native] embedded server stopped: {e}");
            }
        });

        // Wait for legacy daemon socket listener to establish
        let wait_legacy = runtime
            .block_on(async { wait_for_listener(&legacy_socket, Duration::from_secs(5)).await });
        if let Err(e) = wait_legacy {
            server_handle.abort();
            return Err(e);
        }

        let bridge_api = api_socket.clone();
        let bridge_legacy = legacy_socket.clone();
        let bridge_handle = runtime.spawn(async move {
            if let Err(e) = jcode_harness_api_server::run_bridge(bridge_api, bridge_legacy).await {
                eprintln!("[h2o-native] embedded harness bridge stopped: {e}");
            }
        });

        // Wait for Harness API bridge socket listener to establish
        let wait_api = runtime
            .block_on(async { wait_for_listener(&api_socket, Duration::from_secs(5)).await });
        if let Err(e) = wait_api {
            bridge_handle.abort();
            server_handle.abort();
            return Err(e);
        }

        Ok(Self {
            _runtime: runtime,
            temp_dir,
            api_socket,
            server_handle,
            bridge_handle,
        })
    }
}

impl Drop for EmbeddedRuntime {
    fn drop(&mut self) {
        self.bridge_handle.abort();
        self.server_handle.abort();
        // The `temp_dir` is dropped here, removing the temporary directory.
        // On Windows, the OS Named Pipe kernel handles close when the tasks abort.
    }
}

async fn wait_for_listener(path: &Path, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if jcode_app_core::transport::is_socket_path(path) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err(format!(
        "Timed out waiting for socket listener at {}",
        path.display()
    ))
}
