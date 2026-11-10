use h2o_drift_prototype::memory::{Memory, Phase};
use std::{fs, io::ErrorKind};

mod support;
use support::TestDir;

#[test]
fn every_phase_preserves_empty_binary_and_utf8_sources_after_reopen() {
    let root = TestDir::new();
    let sources = [
        Vec::new(),
        (0..=255).collect::<Vec<u8>>(),
        "原文と改行\nsecond line\r\n".as_bytes().to_vec(),
        vec![0xa5; 65_536],
    ];
    for (index, source) in sources.iter().enumerate() {
        let id = format!("source_{index}");
        let mut memory = Memory::open(&root.path).unwrap();
        memory.insert(&id, source).unwrap();
        assert_eq!(memory.resident_bytes(), 0);
        for phase in [Phase::Ice, Phase::Water, Phase::Vapour, Phase::Water] {
            memory.transition(&id, phase).unwrap();
            assert_eq!(memory.get(&id).unwrap(), *source);
            assert_eq!(fs::read(root.path.join(&id)).unwrap(), *source);
            assert_eq!(
                memory.resident_bytes(),
                if phase == Phase::Vapour {
                    0
                } else {
                    source.len()
                }
            );
            let reopened = Memory::open(&root.path).unwrap();
            assert_eq!(reopened.resident_bytes(), 0);
            assert_eq!(reopened.get(&id).unwrap(), *source);
        }
    }
}

#[test]
fn duplicate_ids_report_collision_without_replacing_source_or_cache() {
    let root = TestDir::new();
    let mut first = Memory::open(&root.path).unwrap();
    first.insert("same", b"original").unwrap();
    first.transition("same", Phase::Water).unwrap();
    assert_eq!(
        first.insert("same", b"replacement").unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    let mut second = Memory::open(&root.path).unwrap();
    assert_eq!(
        second.insert("same", b"other writer").unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(first.get("same").unwrap(), b"original");
    assert_eq!(second.get("same").unwrap(), b"original");
    assert_eq!(first.resident_bytes(), b"original".len());
}

#[test]
fn invalid_ids_are_rejected_by_every_operation_without_creating_files() {
    let root = TestDir::new();
    let mut memory = Memory::open(&root.path).unwrap();
    for id in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        "C:drive",
        "white space",
        "日本語",
        "nul\0byte",
    ] {
        assert_eq!(
            memory.insert(id, b"data").unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(memory.get(id).unwrap_err().kind(), ErrorKind::InvalidInput);
        for phase in [Phase::Ice, Phase::Water, Phase::Vapour] {
            assert_eq!(
                memory.transition(id, phase).unwrap_err().kind(),
                ErrorKind::InvalidInput
            );
        }
    }
    assert_eq!(memory.resident_bytes(), 0);
    assert_eq!(fs::read_dir(&root.path).unwrap().count(), 0);
}

#[test]
fn missing_sources_report_failure_without_affecting_other_entries() {
    let root = TestDir::new();
    let mut memory = Memory::open(&root.path).unwrap();
    memory.insert("kept", b"kept source").unwrap();
    memory.transition("kept", Phase::Ice).unwrap();
    assert_eq!(
        memory.get("missing").unwrap_err().kind(),
        ErrorKind::NotFound
    );
    for phase in [Phase::Ice, Phase::Water, Phase::Vapour] {
        assert_eq!(
            memory.transition("missing", phase).unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }
    assert_eq!(memory.get("kept").unwrap(), b"kept source");
    assert_eq!(memory.resident_bytes(), b"kept source".len());
}

#[test]
fn removed_disk_source_is_reported_after_reopen_and_during_restoration() {
    let root = TestDir::new();
    let mut memory = Memory::open(&root.path).unwrap();
    memory.insert("lost", b"source").unwrap();
    drop(memory);
    fs::remove_file(root.path.join("lost")).unwrap();
    let mut reopened = Memory::open(&root.path).unwrap();
    assert_eq!(
        reopened.get("lost").unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        reopened
            .transition("lost", Phase::Water)
            .unwrap_err()
            .kind(),
        ErrorKind::NotFound
    );
    assert_eq!(reopened.resident_bytes(), 0);
}

#[test]
fn resident_payload_accounting_is_per_entry_and_transitions_are_repeatable() {
    let root = TestDir::new();
    let mut memory = Memory::open(&root.path).unwrap();
    memory.insert("A-1", b"abc").unwrap();
    memory.insert("B_2", b"12345").unwrap();
    memory.transition("A-1", Phase::Ice).unwrap();
    memory.transition("B_2", Phase::Water).unwrap();
    assert_eq!(memory.resident_bytes(), 8);
    for _ in 0..3 {
        memory.transition("A-1", Phase::Water).unwrap();
        assert_eq!(memory.resident_bytes(), 8);
        memory.transition("A-1", Phase::Vapour).unwrap();
        assert_eq!(memory.resident_bytes(), 5);
        assert_eq!(memory.get("A-1").unwrap(), b"abc");
        assert_eq!(memory.resident_bytes(), 5);
        memory.transition("A-1", Phase::Ice).unwrap();
    }
    let mut returned = memory.get("B_2").unwrap();
    returned.fill(0);
    assert_eq!(memory.get("B_2").unwrap(), b"12345");
}

#[test]
fn open_reports_a_file_used_as_a_directory() {
    let root = TestDir::new();
    let file = root.path.join("file");
    fs::write(&file, b"unchanged").unwrap();
    assert!(Memory::open(&file).is_err());
    assert_eq!(fs::read(file).unwrap(), b"unchanged");
}
