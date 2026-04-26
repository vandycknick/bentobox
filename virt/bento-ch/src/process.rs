use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

use crate::builder::VirtualMachineBuilder;
use crate::client::CloudHypervisorClient;
use crate::error::CloudHypervisorError;

const DEFAULT_API_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_API_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
enum ProcessExit {
    Running,
    Exited(ExitStatus),
    Failed(String),
}

#[derive(Debug)]
pub struct CloudHypervisorProcessBuilder {
    cloud_hypervisor_bin: PathBuf,
    socket_path: PathBuf,
    api_timeout: Duration,
    api_poll_interval: Duration,
    cleanup_socket: bool,
}

impl CloudHypervisorProcessBuilder {
    pub fn new(cloud_hypervisor_bin: impl Into<PathBuf>, socket_path: impl Into<PathBuf>) -> Self {
        Self {
            cloud_hypervisor_bin: cloud_hypervisor_bin.into(),
            socket_path: socket_path.into(),
            api_timeout: DEFAULT_API_TIMEOUT,
            api_poll_interval: DEFAULT_API_POLL_INTERVAL,
            cleanup_socket: true,
        }
    }

    pub fn api_timeout(mut self, timeout: Duration) -> Self {
        self.api_timeout = timeout;
        self
    }

    pub fn api_poll_interval(mut self, interval: Duration) -> Self {
        self.api_poll_interval = interval;
        self
    }

    pub fn cleanup_socket(mut self, cleanup: bool) -> Self {
        self.cleanup_socket = cleanup;
        self
    }

    fn build_args(&self) -> Vec<String> {
        vec![
            "--api-socket".to_string(),
            format!("path={}", self.socket_path.display()),
        ]
    }

    pub async fn spawn(self) -> Result<CloudHypervisorProcess, CloudHypervisorError> {
        if self.cleanup_socket && self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        let mut child = Command::new(&self.cloud_hypervisor_bin)
            .args(self.build_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(CloudHypervisorError::Spawn)?;

        wait_for_api(
            &self.socket_path,
            self.api_timeout,
            self.api_poll_interval,
            &mut child,
        )
        .await?;

        let pid = child.id();
        let (exit_tx, exit_rx) = watch::channel(ProcessExit::Running);
        tokio::task::spawn_blocking(move || {
            let exit = match child.wait() {
                Ok(status) => ProcessExit::Exited(status),
                Err(err) => ProcessExit::Failed(err.to_string()),
            };
            let _ = exit_tx.send(exit);
        });

        Ok(CloudHypervisorProcess {
            pid,
            socket_path: self.socket_path,
            cleanup_socket_on_drop: self.cleanup_socket,
            exit: exit_rx,
        })
    }
}

#[derive(Debug)]
pub struct CloudHypervisorProcess {
    pid: u32,
    socket_path: PathBuf,
    cleanup_socket_on_drop: bool,
    exit: watch::Receiver<ProcessExit>,
}

impl CloudHypervisorProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn client(&self) -> Result<CloudHypervisorClient, CloudHypervisorError> {
        CloudHypervisorClient::connect(&self.socket_path)
    }

    pub fn builder(&self) -> VirtualMachineBuilder {
        VirtualMachineBuilder::new(&self.socket_path)
    }

    pub async fn shutdown_vmm(&self) -> Result<ExitStatus, CloudHypervisorError> {
        self.client()?.shutdown_vmm().await?;
        self.wait().await
    }

    pub async fn shutdown(&self) -> Result<ExitStatus, CloudHypervisorError> {
        self.shutdown_vmm().await
    }

    pub async fn kill(&self) -> Result<ExitStatus, CloudHypervisorError> {
        signal_child(self.pid, Signal::SIGKILL)?;
        self.wait().await
    }

    pub async fn wait(&self) -> Result<ExitStatus, CloudHypervisorError> {
        let mut exit = self.exit.clone();
        loop {
            if let Some(result) = process_exit_result(&exit.borrow()) {
                return result;
            }

            match exit.changed().await {
                Ok(()) => {}
                Err(_) => {
                    if let Some(result) = process_exit_result(&exit.borrow()) {
                        return result;
                    }
                    return Err(CloudHypervisorError::Io(std::io::Error::other(
                        "cloud-hypervisor process reaper exited before reporting status",
                    )));
                }
            }
        }
    }

    pub fn try_wait(&self) -> Result<Option<ExitStatus>, CloudHypervisorError> {
        match process_exit_result(&self.exit.borrow()) {
            Some(Ok(status)) => Ok(Some(status)),
            Some(Err(err)) => Err(err),
            None => Ok(None),
        }
    }
}

impl Drop for CloudHypervisorProcess {
    fn drop(&mut self) {
        if matches!(*self.exit.borrow(), ProcessExit::Running) {
            let _ = signal_child(self.pid, Signal::SIGKILL);
        }

        if self.cleanup_socket_on_drop {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

async fn wait_for_api(
    path: &Path,
    timeout_duration: Duration,
    poll_interval: Duration,
    child: &mut Child,
) -> Result<(), CloudHypervisorError> {
    let path = path.to_path_buf();
    timeout(timeout_duration, async {
        loop {
            if path.exists() {
                if let Ok(client) =
                    CloudHypervisorClient::connect_with_timeout(&path, poll_interval)
                {
                    if client.ping_vmm().await.is_ok() {
                        return Ok(());
                    }
                }
            }

            if child.try_wait()?.is_some() {
                return Err(CloudHypervisorError::ProcessExited);
            }

            sleep(poll_interval).await;
        }
    })
    .await
    .map_err(|_| CloudHypervisorError::ApiTimeout(path))?
}

fn signal_child(pid: u32, signal: Signal) -> Result<(), CloudHypervisorError> {
    kill(Pid::from_raw(pid as i32), signal).map_err(CloudHypervisorError::Signal)
}

fn process_exit_result(exit: &ProcessExit) -> Option<Result<ExitStatus, CloudHypervisorError>> {
    match exit {
        ProcessExit::Running => None,
        ProcessExit::Exited(status) => Some(Ok(*status)),
        ProcessExit::Failed(err) => Some(Err(CloudHypervisorError::Io(std::io::Error::other(
            err.clone(),
        )))),
    }
}

#[cfg(test)]
mod tests {
    use super::CloudHypervisorProcessBuilder;

    #[test]
    fn build_args_use_path_api_socket() {
        let builder = CloudHypervisorProcessBuilder::new("cloud-hypervisor", "/tmp/ch.sock");
        let args = builder.build_args();

        assert_eq!(args, vec!["--api-socket", "path=/tmp/ch.sock"]);
    }
}
