use eyre::Context;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[must_use = "hold this guard for the process lifetime to keep control socket cleanup active"]
pub struct SocketGuard {
    path: PathBuf,
    pub(crate) listener: tokio::net::UnixListener,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn bind_socket(path: &Path) -> eyre::Result<SocketGuard> {
    if let Err(err) = fs::remove_file(path) {
        if err.kind() != io::ErrorKind::NotFound {
            return Err(err).context(format!("remove stale socket {}", path.display()));
        }
    }

    let listener = std::os::unix::net::UnixListener::bind(path)
        .context(format!("bind socket {}", path.display()))?;
    listener
        .set_nonblocking(true)
        .context("set control socket nonblocking")?;
    let listener = tokio::net::UnixListener::from_std(listener)
        .context("convert control socket to tokio listener")?;

    Ok(SocketGuard {
        path: path.to_path_buf(),
        listener,
    })
}
