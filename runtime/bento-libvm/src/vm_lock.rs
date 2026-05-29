use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use nix::fcntl::{Flock, FlockArg};

pub(crate) struct VmLock {
    _file: Flock<File>,
}

impl VmLock {
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let locked = Flock::lock(file, FlockArg::LockExclusive)
            .map_err(|(_, err)| io::Error::other(err.to_string()))?;
        Ok(Self { _file: locked })
    }
}
