//! A minimal file-backed, reversible working-set prototype.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const PENDING_PREFIX: &str = ".h2o-pending-";
static NEXT_PENDING: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
fn is_windows_device_id(id: &str) -> bool {
    let name = id.to_ascii_uppercase();
    matches!(name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
            })
        })
}

struct PendingFile(PathBuf);

impl PendingFile {
    fn create(root: &Path) -> io::Result<(Self, File)> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        loop {
            let sequence = NEXT_PENDING.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "{PENDING_PREFIX}{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self(path), file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        // Only this writer's staging name is removed. A failed cleanup is visible via pending_files.
        let _ = fs::remove_file(&self.0);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Ice,
    Water,
    Vapour,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitPoint {
    Created,
    Written,
    Synced,
    Published,
}

/// Source bytes are stored on disk before they may be held in the cache.
/// The prototype intentionally does not implement correction, consent or deletion.
pub struct Memory {
    root: PathBuf,
    cache: HashMap<String, Vec<u8>>,
    phases: HashMap<String, Phase>,
}

impl Memory {
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            cache: HashMap::new(),
            phases: HashMap::new(),
        })
    }
    fn path(&self, id: &str) -> io::Result<PathBuf> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid memory ID",
            ));
        }
        #[cfg(windows)]
        if is_windows_device_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reserved Windows device ID",
            ));
        }
        Ok(self.root.join(id))
    }
    pub fn insert(&mut self, id: &str, original: &[u8]) -> io::Result<()> {
        self.insert_observed(id, original, |_| Ok(()))
    }
    fn insert_observed(
        &mut self,
        id: &str,
        original: &[u8],
        mut observe: impl FnMut(CommitPoint) -> io::Result<()>,
    ) -> io::Result<()> {
        let path = self.path(id)?;
        let (pending, mut f) = PendingFile::create(&self.root)?;
        let result: io::Result<()> = (|| {
            observe(CommitPoint::Created)?;
            for chunk in original.chunks(64 * 1024) {
                f.write_all(chunk)?;
                observe(CommitPoint::Written)?;
            }
            f.sync_all()?;
            observe(CommitPoint::Synced)?;
            Ok(())
        })();
        drop(f);
        result?;
        // Same-filesystem hard links publish complete bytes without replacing an existing ID.
        // Unsupported filesystems fail here; never fall back to an overwriting rename or copy.
        fs::hard_link(&pending.0, &path)?;
        observe(CommitPoint::Published)?;
        self.phases.insert(id.to_owned(), Phase::Vapour);
        Ok(())
    }
    /// Unpublished or leftover staging names. May belong to live writers; never delete automatically.
    pub fn pending_files(&self) -> io::Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(PENDING_PREFIX)
            {
                paths.push(entry.path());
            }
        }
        paths.sort();
        Ok(paths)
    }
    pub fn transition(&mut self, id: &str, phase: Phase) -> io::Result<()> {
        let path = self.path(id)?;
        match phase {
            Phase::Ice | Phase::Water => {
                let original = fs::read(path)?;
                self.cache.insert(id.to_owned(), original);
            }
            Phase::Vapour => {
                fs::metadata(path)?;
                self.cache.remove(id);
            }
        }
        self.phases.insert(id.to_owned(), phase);
        Ok(())
    }
    pub fn get(&self, id: &str) -> io::Result<Vec<u8>> {
        let path = self.path(id)?;
        match self.cache.get(id) {
            Some(data) => Ok(data.clone()),
            None => fs::read(path),
        }
    }
    pub fn resident_bytes(&self) -> usize {
        self.cache.values().map(Vec::len).sum()
    }
}

#[cfg(test)]
mod persistence_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn reserved_device_names_are_rejected_before_any_file_operation() {
        let memory = Memory::open(std::env::temp_dir()).unwrap();
        for id in [
            "CON", "con", "NUL", "nul", "PrN", "AUX", "COM1", "com9", "LPT1", "lpt9",
        ] {
            assert_eq!(
                memory.path(id).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "accepted {id}"
            );
        }
    }
    #[test]
    fn transitions_restore_exact_bytes_and_reopen() {
        let root = std::env::temp_dir().join(format!(
            "h2o-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let original = b"original\0\xff";
        let mut m = Memory::open(&root).unwrap();
        m.insert("one", original).unwrap();
        m.transition("one", Phase::Ice).unwrap();
        assert_eq!(m.resident_bytes(), original.len());
        m.transition("one", Phase::Vapour).unwrap();
        assert_eq!(m.resident_bytes(), 0);
        assert_eq!(m.get("one").unwrap(), original);
        assert!(m.insert("one", b"overwrite").is_err());
        drop(m);
        let m = Memory::open(&root).unwrap();
        assert_eq!(m.get("one").unwrap(), original);
        assert!(m.get("../bad").is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
