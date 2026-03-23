use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use tokio::time::{sleep, timeout};

use crate::builder::VirtualMachineBuilder;
use crate::client::FirecrackerClient;
use crate::error::FirecrackerError;
use crate::serial::SerialConnection;

const DEFAULT_SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct FirecrackerProcessBuilder {
    firecracker_bin: PathBuf,
    socket_path: PathBuf,
    id: Option<String>,
    socket_timeout: Duration,
    socket_poll_interval: Duration,
    cleanup_socket: bool,
}

impl FirecrackerProcessBuilder {
    pub fn new(firecracker_bin: impl Into<PathBuf>, socket_path: impl Into<PathBuf>) -> Self {
        Self {
            firecracker_bin: firecracker_bin.into(),
            socket_path: socket_path.into(),
            id: None,
            socket_timeout: DEFAULT_SOCKET_TIMEOUT,
            socket_poll_interval: DEFAULT_SOCKET_POLL_INTERVAL,
            cleanup_socket: true,
        }
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn socket_timeout(mut self, timeout: Duration) -> Self {
        self.socket_timeout = timeout;
        self
    }

    pub fn socket_poll_interval(mut self, interval: Duration) -> Self {
        self.socket_poll_interval = interval;
        self
    }

    pub fn cleanup_socket(mut self, cleanup: bool) -> Self {
        self.cleanup_socket = cleanup;
        self
    }

    fn build_args(&self) -> Vec<String> {
        let mut args = vec![
            "--api-sock".to_string(),
            self.socket_path.display().to_string(),
        ];

        if let Some(id) = &self.id {
            args.push("--id".to_string());
            args.push(id.clone());
        }

        args
    }

    pub async fn spawn(self) -> Result<FirecrackerProcess, FirecrackerError> {
        if self.cleanup_socket && self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        let mut child = Command::new(&self.firecracker_bin)
            .args(self.build_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(FirecrackerError::Spawn)?;

        wait_for_socket(
            &self.socket_path,
            self.socket_timeout,
            self.socket_poll_interval,
            &mut child,
        )
        .await?;

        let serial_write = File::from(std::os::fd::OwnedFd::from(
            child
                .stdin
                .take()
                .ok_or(FirecrackerError::MissingChildPipe("stdin"))?,
        ));
        let serial_read = File::from(std::os::fd::OwnedFd::from(
            child
                .stdout
                .take()
                .ok_or(FirecrackerError::MissingChildPipe("stdout"))?,
        ));

        let pid = child.id();
        Ok(FirecrackerProcess {
            child: Arc::new(Mutex::new(child)),
            pid,
            socket_path: self.socket_path,
            serial_read,
            serial_write,
            cleanup_socket_on_drop: self.cleanup_socket,
        })
    }
}

pub struct FirecrackerProcess {
    child: Arc<Mutex<Child>>,
    pid: u32,
    socket_path: PathBuf,
    serial_read: File,
    serial_write: File,
    cleanup_socket_on_drop: bool,
}

impl FirecrackerProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn client(&self) -> Result<FirecrackerClient, FirecrackerError> {
        FirecrackerClient::connect(&self.socket_path)
    }

    pub fn builder(&self) -> VirtualMachineBuilder {
        VirtualMachineBuilder::new(&self.socket_path)
    }

    pub fn serial(&self) -> Result<SerialConnection, FirecrackerError> {
        SerialConnection::from_cloned_files(&self.serial_read, &self.serial_write)
    }

    pub async fn shutdown(&self) -> Result<ExitStatus, FirecrackerError> {
        signal_child(self.pid, Signal::SIGTERM)?;
        self.wait().await
    }

    pub async fn kill(&self) -> Result<ExitStatus, FirecrackerError> {
        signal_child(self.pid, Signal::SIGKILL)?;
        self.wait().await
    }

    pub async fn wait(&self) -> Result<ExitStatus, FirecrackerError> {
        let child = self.child.clone();
        tokio::task::spawn_blocking(move || {
            let mut child = child
                .lock()
                .map_err(|_| FirecrackerError::ChildHandlePoisoned)?;
            child.wait().map_err(FirecrackerError::Io)
        })
        .await
        .map_err(|err| FirecrackerError::Io(std::io::Error::other(err.to_string())))?
    }
}

impl Drop for FirecrackerProcess {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            if let Ok(None) = child.try_wait() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        if self.cleanup_socket_on_drop {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

async fn wait_for_socket(
    path: &Path,
    timeout_duration: Duration,
    poll_interval: Duration,
    child: &mut Child,
) -> Result<(), FirecrackerError> {
    let path = path.to_path_buf();
    timeout(timeout_duration, async {
        loop {
            if path.exists() && tokio::net::UnixStream::connect(&path).await.is_ok() {
                return Ok(());
            }

            if child.try_wait()?.is_some() {
                return Err(FirecrackerError::ProcessExited);
            }

            sleep(poll_interval).await;
        }
    })
    .await
    .map_err(|_| FirecrackerError::SocketTimeout(path))?
}

fn signal_child(pid: u32, signal: Signal) -> Result<(), FirecrackerError> {
    kill(Pid::from_raw(pid as i32), signal).map_err(FirecrackerError::Signal)
}

#[cfg(test)]
mod tests {
    use super::FirecrackerProcessBuilder;

    #[test]
    fn firecracker_builder_includes_id_when_present() {
        let builder =
            FirecrackerProcessBuilder::new("firecracker", "/tmp/firecracker.sock").id("my-vm");

        let args = builder.build_args();
        assert_eq!(args[0], "--api-sock");
        assert_eq!(args[1], "/tmp/firecracker.sock");
        assert!(args.contains(&"--id".to_string()));
        assert!(args.contains(&"my-vm".to_string()));
    }
}
