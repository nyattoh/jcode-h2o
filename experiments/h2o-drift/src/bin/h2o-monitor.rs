//! Attach the drift monitor to one worker of a running jcode daemon and judge it with Jev.
//! Never starts a daemon or runtime. The API key is read by `JevJudge` only and never printed.
use h2o_drift_prototype::{
    jev::{JevConfig, JevJudge},
    memory::Memory,
    monitor::BranchSpec,
    run::{RunEnd, run},
};
use jcode_sdk::{ConnectOptions, JcodeClient};
use std::{collections::HashMap, fs, path::PathBuf, process::ExitCode, time::Duration};

const USAGE: &str = "usage: h2o-monitor --enable-jev --socket <path> --spec <json> --journal <dir> \
--sources <dir> --owner <session> --worker <session> [--jev-base-url <url>] [--min-probability <p>] \
[--deadline-ms <ms>] [--output-cap-bytes <n>] [--include-context-bytes] [--poll-ms <ms>]";

const FLAGS: [&str; 2] = ["--enable-jev", "--include-context-bytes"];
const VALUES: [&str; 11] = [
    "--socket",
    "--spec",
    "--journal",
    "--sources",
    "--owner",
    "--worker",
    "--jev-base-url",
    "--min-probability",
    "--deadline-ms",
    "--output-cap-bytes",
    "--poll-ms",
];

fn parse(args: impl Iterator<Item = String>) -> Result<Option<HashMap<String, String>>, String> {
    let mut map = HashMap::new();
    let mut args = args;
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "-h" | "--help") {
            return Ok(None);
        }
        let value = if FLAGS.contains(&arg.as_str()) {
            String::new()
        } else if VALUES.contains(&arg.as_str()) {
            args.next().ok_or(format!("{arg} needs a value"))?
        } else {
            return Err(format!("unknown argument {arg}"));
        };
        if map.insert(arg.clone(), value).is_some() {
            return Err(format!("{arg} given twice"));
        }
    }
    Ok(Some(map))
}

fn number<T: std::str::FromStr>(
    args: &HashMap<String, String>,
    name: &str,
) -> Result<Option<T>, String> {
    args.get(name)
        .map(|v| v.parse().map_err(|_| format!("{name}: invalid number")))
        .transpose()
}

fn main_inner() -> Result<Option<(RunEnd, u64)>, String> {
    let Some(args) = parse(std::env::args().skip(1))? else {
        return Ok(None);
    };
    if !args.contains_key("--enable-jev") {
        return Err("--enable-jev is required: this sends goal and tool output to Jev".into());
    }
    let need = |name: &str| args.get(name).cloned().ok_or(format!("{name} is required"));
    let (socket, spec_path, journal, sources) = (
        PathBuf::from(need("--socket")?),
        need("--spec")?,
        PathBuf::from(need("--journal")?),
        PathBuf::from(need("--sources")?),
    );
    let (owner, worker) = (need("--owner")?, need("--worker")?);
    let spec: BranchSpec = serde_json::from_slice(
        &fs::read(&spec_path).map_err(|e| format!("cannot read spec: {e}"))?,
    )
    .map_err(|e| format!("invalid spec: {e}"))?;
    if spec.owner_session_id != owner || spec.session_id != worker {
        return Err("--owner/--worker do not match spec owner_session_id/session_id".into());
    }
    if !sources.is_dir() {
        return Err("--sources must be an existing directory".into());
    }

    let mut config = JevConfig {
        enabled: true,
        include_context_bytes: args.contains_key("--include-context-bytes"),
        ..JevConfig::default()
    };
    if let Some(url) = args.get("--jev-base-url") {
        config.base_url = url.clone();
    }
    if let Some(p) = number(&args, "--min-probability")? {
        config.min_probability = p;
    }
    if let Some(ms) = number(&args, "--deadline-ms")? {
        config.deadline = Duration::from_millis(ms);
    }
    if let Some(n) = number(&args, "--output-cap-bytes")? {
        config.output_cap_bytes = n;
    }
    let poll = Duration::from_millis(number(&args, "--poll-ms")?.unwrap_or(200));
    let mut judge = JevJudge::new(config).map_err(|e| format!("jev config: {e}"))?;
    let sources = Memory::open(&sources).map_err(|e| format!("sources: {e}"))?;

    let client = JcodeClient::connect(ConnectOptions {
        socket_path: Some(socket),
        ensure_runtime: false,
        client_name: "h2o-monitor".into(),
        ..ConnectOptions::default()
    })
    .map_err(|e| format!("connect: {e}"))?;
    run(client, spec, sources, &journal, &mut judge, poll)
        .map(Some)
        .map_err(|e| format!("monitor: {e}"))
}

fn main() -> ExitCode {
    match main_inner() {
        Ok(None) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Some((end, checkpoints))) => {
            println!("{end:?} checkpoints={checkpoints}");
            // Uncertain control must not look like a clean finish to a calling script.
            match end {
                RunEnd::NeedsReconciliation => ExitCode::from(3),
                RunEnd::Stopped | RunEnd::Disconnected => ExitCode::SUCCESS,
            }
        }
        Err(message) => {
            eprintln!("h2o-monitor: {message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
