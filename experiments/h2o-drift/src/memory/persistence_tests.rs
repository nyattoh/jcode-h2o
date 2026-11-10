use super::*;

#[path = "../../tests/support/mod.rs"]
mod support;
use std::{
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use support::TestDir;

const SOURCE: &[u8] = &[0xa5; 131_073];

struct ChildGuard(Child);

impl ChildGuard {
    fn wait(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "child failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "child did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child(root: &Path, mode: &str, label: &str) -> ChildGuard {
    ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "memory::persistence_tests::subprocess_entry",
            ])
            .env("H2O_TEST_ROOT", root)
            .env("H2O_TEST_MODE", mode)
            .env("H2O_TEST_LABEL", label)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "child checkpoint not reached: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn readers_never_observe_uncommitted_bytes() {
    let root = TestDir::new();
    let mut writer = Memory::open(&root.path).unwrap();
    let reader = Memory::open(&root.path).unwrap();
    let source = vec![0xa5; 131_073];
    writer
        .insert_observed("source", &source, |point| {
            if point == CommitPoint::Published {
                assert_eq!(reader.get("source").unwrap(), source);
            } else {
                assert_eq!(
                    reader.get("source").unwrap_err().kind(),
                    io::ErrorKind::NotFound
                );
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn write_failures_do_not_publish_and_leave_existing_sources_intact() {
    let root = TestDir::new();
    let mut memory = Memory::open(&root.path).unwrap();
    memory.insert("kept", b"original").unwrap();
    for failure in [
        CommitPoint::Created,
        CommitPoint::Written,
        CommitPoint::Synced,
    ] {
        let error = memory
            .insert_observed("new", SOURCE, |point| {
                if point == failure {
                    Err(io::Error::other("injected write failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            memory.get("new").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(memory.get("kept").unwrap(), b"original");
        assert!(memory.pending_files().unwrap().is_empty());
    }
    memory.insert("new", SOURCE).unwrap();
    assert_eq!(memory.get("new").unwrap(), SOURCE);
}

#[test]
fn killed_writer_exposes_either_no_source_or_the_complete_source_after_restart() {
    for point in ["Created", "Written", "Synced", "Published"] {
        let root = TestDir::new();
        let mut writer = child(&root.path, "pause", point);
        wait_for(&root.path.join("ready"));
        writer.0.kill().unwrap();
        assert!(!writer.0.wait().unwrap().success());
        let mut memory = Memory::open(root.path.join("store")).unwrap();
        assert_eq!(memory.pending_files().unwrap().len(), 1);
        if point == "Published" {
            assert_eq!(memory.get("source").unwrap(), SOURCE);
            assert_eq!(
                memory.insert("source", b"replacement").unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
        } else {
            assert_eq!(
                memory.get("source").unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
            memory.insert("source", SOURCE).unwrap();
        }
        let mut reader = child(&root.path, "read", "reader");
        reader.wait();
        assert_eq!(memory.pending_files().unwrap().len(), 1);
    }
}

#[test]
fn concurrent_processes_publish_exactly_one_source_without_overwrite() {
    let root = TestDir::new();
    let mut first = child(&root.path, "race", "a");
    let mut second = child(&root.path, "race", "b");
    wait_for(&root.path.join("ready-a"));
    wait_for(&root.path.join("ready-b"));
    fs::write(root.path.join("go"), []).unwrap();
    first.wait();
    second.wait();
    let results = [
        fs::read(root.path.join("result-a")).unwrap(),
        fs::read(root.path.join("result-b")).unwrap(),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|r| r.as_slice() == b"committed")
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| r.as_slice() == b"collision")
            .count(),
        1
    );
    let memory = Memory::open(root.path.join("store")).unwrap();
    let winner = if results[0] == b"committed" {
        b'a'
    } else {
        b'b'
    };
    assert_eq!(memory.get("source").unwrap(), vec![winner; SOURCE.len()]);
    assert!(memory.pending_files().unwrap().is_empty());
}

#[test]
fn completed_write_is_restored_by_a_separate_process() {
    let root = TestDir::new();
    child(&root.path, "write", "writer").wait();
    child(&root.path, "read", "reader").wait();
}

#[test]
#[ignore = "subprocess fixture; invoked explicitly by parent tests"]
fn subprocess_entry() {
    let root = PathBuf::from(std::env::var_os("H2O_TEST_ROOT").unwrap());
    let mode = std::env::var("H2O_TEST_MODE").unwrap();
    let label = std::env::var("H2O_TEST_LABEL").unwrap();
    let mut memory = Memory::open(root.join("store")).unwrap();
    match mode.as_str() {
        "pause" => memory
            .insert_observed("source", SOURCE, |point| {
                if format!("{point:?}") == label {
                    fs::write(root.join("ready"), []).unwrap();
                    wait_for(&root.join("never-released"));
                }
                Ok(())
            })
            .unwrap(),
        "race" => {
            fs::write(root.join(format!("ready-{label}")), []).unwrap();
            wait_for(&root.join("go"));
            let bytes = vec![label.as_bytes()[0]; SOURCE.len()];
            let result = match memory.insert("source", &bytes) {
                Ok(()) => "committed",
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => "collision",
                Err(error) => panic!("unexpected write failure: {error}"),
            };
            fs::write(root.join(format!("result-{label}")), result).unwrap();
        }
        "write" => memory.insert("source", SOURCE).unwrap(),
        "read" => assert_eq!(memory.get("source").unwrap(), SOURCE),
        _ => panic!("unknown child mode"),
    }
}
