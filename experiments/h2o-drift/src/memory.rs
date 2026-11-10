//! A minimal file-backed, reversible working-set prototype.
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Ice,
    Water,
    Vapour,
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
        Ok(self.root.join(id))
    }
    pub fn insert(&mut self, id: &str, original: &[u8]) -> io::Result<()> {
        let path = self.path(id)?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        if let Err(e) = f.write_all(original).and_then(|_| f.sync_all()) {
            drop(f);
            let _ = fs::remove_file(path);
            return Err(e);
        }
        self.phases.insert(id.to_owned(), Phase::Vapour);
        Ok(())
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
mod tests {
    use super::*;
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
