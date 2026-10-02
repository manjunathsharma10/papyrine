use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Where `History` parks old entries' images. The host decides (the journal directory, a temp
/// dir, memory for tests).
pub trait SpillStore: Send {
    fn put(&mut self, key: u64, bytes: &[u8]) -> io::Result<()>;
    fn get(&mut self, key: u64) -> io::Result<Vec<u8>>;
    fn remove(&mut self, key: u64) -> io::Result<()>;
}

/// One file per entry under a directory.
pub struct DirStore {
    dir: PathBuf,
}

impl DirStore {
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(DirStore { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: u64) -> PathBuf {
        self.dir.join(format!("{key:016x}.hist"))
    }
}

impl SpillStore for DirStore {
    fn put(&mut self, key: u64, bytes: &[u8]) -> io::Result<()> {
        let tmp = self.dir.join(format!("{key:016x}.tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, self.path(key))
    }
    fn get(&mut self, key: u64) -> io::Result<Vec<u8>> {
        std::fs::read(self.path(key))
    }
    fn remove(&mut self, key: u64) -> io::Result<()> {
        match std::fs::remove_file(self.path(key)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// In-memory store (tests, or hosts with no disk).
#[derive(Default)]
pub struct MemStore(pub HashMap<u64, Vec<u8>>);

impl SpillStore for MemStore {
    fn put(&mut self, key: u64, bytes: &[u8]) -> io::Result<()> {
        self.0.insert(key, bytes.to_vec());
        Ok(())
    }
    fn get(&mut self, key: u64) -> io::Result<Vec<u8>> {
        self.0
            .get(&key)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such entry"))
    }
    fn remove(&mut self, key: u64) -> io::Result<()> {
        self.0.remove(&key);
        Ok(())
    }
}
