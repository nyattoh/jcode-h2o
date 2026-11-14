//! Serves the local reception UI on 127.0.0.1 only (loopback TCP; no external communication). No daemon, no model, no browser.
//! `--snapshot` is an explicit, read-only, validated import; nothing is read from the journal.
use h2o_drift_prototype::ui::{Launcher, StartError};
use std::{collections::HashMap, path::PathBuf, process::ExitCode};

const USAGE: &str = "usage: h2o-ui [--port <0-65535>] [--assets <dir>] [--snapshot <file>]";
const VALUES: [&str; 3] = ["--port", "--assets", "--snapshot"];

fn parse(args: impl Iterator<Item = String>) -> Result<Option<HashMap<String, String>>, String> {
    let mut map = HashMap::new();
    let mut args = args;
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "-h" | "--help") {
            return Ok(None);
        }
        if !VALUES.contains(&arg.as_str()) {
            return Err(format!("unknown argument {arg}"));
        }
        let value = args.next().ok_or(format!("{arg} needs a value"))?;
        if map.insert(arg.clone(), value).is_some() {
            return Err(format!("{arg} given twice"));
        }
    }
    Ok(Some(map))
}

fn main() -> ExitCode {
    let usage = |message: String| {
        eprintln!("h2o-ui: {message}\n{USAGE}");
        ExitCode::from(2)
    };
    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => return usage(message),
    };
    let port = match args.get("--port").map(|p| p.parse::<u16>()) {
        None => 8787,
        Some(Ok(port)) => port,
        Some(Err(_)) => return usage("--port: invalid port".into()),
    };
    let assets = args
        .get("--assets")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/ui")));
    if !assets.is_dir() {
        return usage("--assets must be an existing directory".into());
    }
    let snapshot = args.get("--snapshot").map(PathBuf::from);
    match Launcher::start(assets, snapshot.as_deref(), port) {
        Ok(launcher) => {
            println!("http://127.0.0.1:{}/", launcher.addr().port());
            // Serve until the process is killed.
            loop {
                std::thread::park();
            }
        }
        Err(StartError::Snapshot(e)) => usage(e.to_string()),
        Err(e @ StartError::Io(_)) => {
            eprintln!("h2o-ui: {e}");
            ExitCode::from(1)
        }
    }
}
