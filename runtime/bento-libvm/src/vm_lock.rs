use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

pub(crate) struct VmLock {
    _file: Flock<File>,
}

impl VmLock {
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        let file = open_lock_file(path)?;
        let locked =
            Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, err)| lock_error(err))?;
        Ok(Self { _file: locked })
    }

    pub(crate) fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        let file = open_lock_file(path)?;
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(locked) => Ok(Some(Self { _file: locked })),
            Err((_, err)) if err == Errno::EAGAIN || err == Errno::EWOULDBLOCK => Ok(None),
            Err((_, err)) => Err(lock_error(err)),
        }
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn lock_error(err: Errno) -> io::Error {
    io::Error::other(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::VmLock;

    #[test]
    fn try_acquire_returns_none_when_lock_is_held() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let path = temp.path().join("vm.lock");
        let _lock = VmLock::acquire(&path).expect("acquire lock");

        assert!(VmLock::try_acquire(&path)
            .expect("try acquire lock")
            .is_none());
    }

    #[test]
    fn try_acquire_returns_lock_when_available() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let path = temp.path().join("vm.lock");

        assert!(VmLock::try_acquire(&path)
            .expect("try acquire lock")
            .is_some());
    }
}
