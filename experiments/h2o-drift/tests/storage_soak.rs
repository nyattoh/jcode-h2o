use h2o_drift_prototype::memory::Memory;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

mod support;
use support::TestDir;

fn payload(seed: u64, writer: u8) -> Vec<u8> {
    let size = match seed % 8 {
        0 => 0,
        1 => 1,
        2 => 65_535,
        3 => 65_536,
        4 => 65_537,
        5 => 131_073,
        _ => (seed % 32_768) as usize,
    };
    let mut state = seed ^ ((writer as u64 + 1) << 32);
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "soak child stalled: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(1));
    }
}

struct Process(Child);
impl Process {
    fn start(root: &Path, seed: u64, label: &str) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "subprocess_entry"])
                .env("H2O_SOAK_ROOT", root)
                .env("H2O_SOAK_SEED", seed.to_string())
                .env("H2O_SOAK_LABEL", label)
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "soak child failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "soak child did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "opt-in, duration-bounded, distinct seeds and process interleavings"]
fn concurrent_publication_soak() {
    let seconds = std::env::var("H2O_SOAK_SECONDS")
        .ok()
        .map(|s| s.parse::<u64>().unwrap())
        .unwrap_or(5);
    assert!((1..=1800).contains(&seconds));
    let workspace_parent =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/h2o-soak-fixtures");
    let start = Instant::now();
    let duration = Duration::from_secs(seconds);
    let mut cases = 0u64;
    let mut read_probes = 0u64;
    let mut attempted_payload_bytes = 0u64;
    let mut last_report = Instant::now();
    while start.elapsed() < duration {
        let seed = 0x9e37_79b9u64.wrapping_mul(cases + 1);
        let root = if cases % 2 == 0 {
            TestDir::new()
        } else {
            TestDir::new_in(&workspace_parent)
        };
        Memory::open(root.path.join("store")).unwrap();
        let mut reader = Process::start(&root.path, seed, "reader");
        let mut writers: Vec<_> = (0..3)
            .map(|index| Process::start(&root.path, seed, &index.to_string()))
            .collect();
        for label in ["reader", "0", "1", "2"] {
            wait_for(&root.path.join(format!("ready-{label}")));
        }
        fs::write(root.path.join("go"), []).unwrap();
        thread::sleep(Duration::from_micros(seed % 3000));
        let killed = (seed % 3) as usize;
        let _ = writers[killed].0.kill();
        let _ = writers[killed].0.wait();
        for (index, writer) in writers.iter_mut().enumerate() {
            if index != killed {
                writer.finish();
            }
        }
        fs::write(root.path.join("stop-reader"), []).unwrap();
        reader.finish();
        let source = Memory::open(root.path.join("store"))
            .unwrap()
            .get("entry")
            .unwrap();
        assert!(
            (0..3).any(|writer| source == payload(seed, writer)),
            "not a complete candidate at seed {seed}"
        );
        let acknowledgements = (0..3)
            .filter(|index| {
                fs::read(root.path.join(format!("result-{index}")))
                    .ok()
                    .as_deref()
                    == Some(b"committed")
            })
            .count();
        assert!(acknowledgements <= 1, "multiple commits at seed {seed}");
        assert!(
            fs::metadata(root.path.join("store/entry"))
                .unwrap()
                .is_file()
        );
        read_probes += fs::read_to_string(root.path.join("reader-probes"))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        attempted_payload_bytes += payload(seed, 0).len() as u64 * 3;
        cases += 1;
        if last_report.elapsed() >= Duration::from_secs(30) {
            println!(
                "soak progress: cases={cases}, read_probes={read_probes}, attempted_payload_bytes={attempted_payload_bytes}, elapsed_seconds={:.1}",
                start.elapsed().as_secs_f64()
            );
            last_report = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(cases > 0);
    println!(
        "soak complete: cases={cases}, read_probes={read_probes}, attempted_payload_bytes={attempted_payload_bytes}, elapsed_seconds={:.1}",
        start.elapsed().as_secs_f64()
    );
}

#[test]
#[ignore = "subprocess fixture for the opt-in soak"]
fn subprocess_entry() {
    let root = PathBuf::from(std::env::var_os("H2O_SOAK_ROOT").unwrap());
    let seed = std::env::var("H2O_SOAK_SEED")
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let label = std::env::var("H2O_SOAK_LABEL").unwrap();
    let mut memory = Memory::open(root.join("store")).unwrap();
    fs::write(root.join(format!("ready-{label}")), []).unwrap();
    wait_for(&root.join("go"));
    if label == "reader" {
        let candidates: Vec<_> = (0..3).map(|writer| payload(seed, writer)).collect();
        let mut observed = None;
        let mut probes = 0u64;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match memory.get("entry") {
                Ok(bytes) => {
                    assert!(
                        candidates.contains(&bytes),
                        "partial or foreign bytes at seed {seed}"
                    );
                    if let Some(previous) = &observed {
                        assert_eq!(&bytes, previous, "published source changed");
                    }
                    observed = Some(bytes);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound && observed.is_none() => {}
                Err(error) => panic!("source disappeared or read failed: {error}"),
            }
            probes += 1;
            if root.join("stop-reader").exists() {
                break;
            }
            assert!(Instant::now() < deadline, "reader did not receive stop");
            thread::yield_now();
        }
        fs::write(root.join("reader-probes"), probes.to_string()).unwrap();
    } else {
        let writer = label.parse::<u8>().unwrap();
        let result = match memory.insert("entry", &payload(seed, writer)) {
            Ok(()) => "committed",
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => "collision",
            Err(error) => panic!("unexpected insertion failure: {error}"),
        };
        fs::write(root.join(format!("result-{label}")), result).unwrap();
    }
}
