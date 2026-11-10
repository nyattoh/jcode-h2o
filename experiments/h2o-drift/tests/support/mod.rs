use std::{fs, path::PathBuf, time::SystemTime};

pub struct TestDir {
    pub path: PathBuf,
    parent: PathBuf,
}

impl TestDir {
    pub fn new() -> Self {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for attempt in 0..100 {
            let path = parent.join(format!(
                "h2o-contract-{}-{nonce}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self { path, parent },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
        panic!("cannot allocate a unique test directory");
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let resolved = self.path.canonicalize().unwrap();
        assert_eq!(resolved.parent(), Some(self.parent.as_path()));
        assert!(
            resolved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("h2o-contract-")
        );
        fs::remove_dir_all(resolved).unwrap();
    }
}
