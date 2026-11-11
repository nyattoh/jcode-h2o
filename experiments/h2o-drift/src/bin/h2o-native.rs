//! Native (egui) reception UI. Demo/unconnected: no daemon, model, Jev or network.
//! Usage: h2o-native [--snapshot <file>]
use eframe::egui;
use h2o_drift_prototype::native_ui::{NativeApp, install_japanese_font};
use std::{path::PathBuf, process::ExitCode};

const USAGE: &str = "usage: h2o-native [--snapshot <file>]";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let mut snapshot: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
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
            .with_title("H₂O 受付（デモ・未接続）")
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
            eprintln!("h2o-native: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
