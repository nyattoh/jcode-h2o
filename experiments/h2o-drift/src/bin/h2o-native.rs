#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Native (egui) reception UI. Starts unconnected in real-chat mode: it talks to an EXISTING
//! jcode harness API socket, or starts the in-process embedded runtime when left blank, only
//! after the user clicks 接続 (never starts a daemon or connects on app launch). The fixed demo is a
//! separate, explicitly chosen mode. Usage: h2o-native [--snapshot <file>]
use eframe::egui;
use h2o_drift_prototype::native_ui::{NativeApp, install_japanese_font};
use std::{path::PathBuf, process::ExitCode};

const USAGE: &str = "usage: h2o-native [--snapshot <file>]";

#[cfg(windows)]
unsafe extern "system" {
    fn AttachConsole(dwProcessId: u32) -> i32;
    fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
}

#[cfg(windows)]
fn attach_console_if_needed() {
    unsafe {
        // STD_OUTPUT_HANDLE = ((DWORD)-11) = 0xFFFF_FFF5
        let stdout_handle = GetStdHandle(0xFFFF_FFF5);
        if stdout_handle.is_null() || stdout_handle == (-1isize as *mut std::ffi::c_void) {
            // ATTACH_PARENT_PROCESS = ((DWORD)-1) = u32::MAX
            let _ = AttachConsole(u32::MAX);
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let mut snapshot: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                #[cfg(windows)]
                attach_console_if_needed();
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some("--snapshot") if snapshot.is_none() => match args.next() {
                Some(p) => snapshot = Some(p.into()),
                None => return usage_error(),
            },
            _ => return usage_error(),
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("H₂O 受付")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([360.0, 480.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "h2o-native",
        options,
        Box::new(move |cc| {
            let mut app = NativeApp::new(install_japanese_font(&cc.egui_ctx));
            if let Some(path) = snapshot {
                app.import_snapshot_file(&path); // errors are shown in the monitor panel
            }
            Ok(Box::new(app))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            #[cfg(windows)]
            attach_console_if_needed();
            eprintln!("h2o-native: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error() -> ExitCode {
    #[cfg(windows)]
    attach_console_if_needed();
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
