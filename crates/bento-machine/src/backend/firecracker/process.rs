use std::fs::File;
use std::io;
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use tokio::sync::watch;

use crate::types::{MachineError, MachineState};

pub(super) const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const STOP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct FirecrackerRuntime {
    pub(super) running: Option<RunningFirecracker>,
}

pub(super) struct RunningFirecracker {
    pub(super) child: Arc<Mutex<Child>>,
    pub(super) serial_read: File,
    pub(super) serial_write: File,
    pub(super) exit_watcher: Option<JoinHandle<()>>,
}

pub(super) fn terminate_child(child: &mut Child) -> Result<(), MachineError> {
    if try_wait_child(child)?.is_some() {
        return Ok(());
    }

    let pid = child.id();
    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        if try_wait_child(child)?.is_some() {
            return Ok(());
        }

        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait()?;
            return Ok(());
        }

        thread::sleep(EXIT_POLL_INTERVAL);
    }
}

pub(super) fn spawn_exit_watcher(
    machine_id: String,
    child: Arc<Mutex<Child>>,
    state: Arc<Mutex<MachineState>>,
    state_tx: watch::Sender<MachineState>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("firecracker-machine-state:{machine_id}"))
        .spawn(move || loop {
            let status = match child.lock() {
                Ok(mut child) => try_wait_child(&mut child),
                Err(_) => return,
            };

            match status {
                Ok(Some(status)) => {
                    if let Ok(mut current_state) = state.lock() {
                        *current_state = MachineState::Stopped;
                    }
                    let _ = state_tx.send(MachineState::Stopped);
                    tracing::info!(machine_id, status = %format_exit_message(status), "firecracker process exited");
                    return;
                }
                Ok(None) => thread::sleep(EXIT_POLL_INTERVAL),
                Err(err) => {
                    if let Ok(mut current_state) = state.lock() {
                        *current_state = MachineState::Stopped;
                    }
                    let _ = state_tx.send(MachineState::Stopped);
                    tracing::warn!(machine_id, error = %err, "failed to poll firecracker process status");
                    return;
                }
            }
        })
        .expect("firecracker exit watcher thread should spawn")
}

pub(super) fn format_exit_message(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("firecracker exited with status code {code}"),
        None => "firecracker exited due to signal".to_string(),
    }
}

pub(super) fn try_wait_child(child: &mut Child) -> Result<Option<ExitStatus>, MachineError> {
    match child.try_wait() {
        Ok(status) => Ok(status),
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(err) => Err(MachineError::Io(err)),
    }
}
