use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam::channel::Receiver;
use tokio::sync::oneshot;

use crate::backend::vz::config::VmBootstrap;
use crate::backend::vz::vm::{VirtualMachine, VirtualMachineState};
use crate::types::{MachineError, MachineExitEvent, MachineState};

pub(super) unsafe fn build_vm(bootstrap: VmBootstrap) -> VirtualMachine {
    VirtualMachine::new(
        bootstrap.config,
        bootstrap.serial.guest_input,
        bootstrap.serial.guest_output,
    )
}

pub(super) async unsafe fn start_vm(vm: VirtualMachine) -> Result<VirtualMachine, MachineError> {
    vm.start().await?;

    let events = vm.subscribe_state();
    let startup_timeout = Duration::from_secs(60 * 5);
    loop {
        let event = match events.recv_timeout(startup_timeout) {
            Ok(event) => event,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                return Err(MachineError::Backend(format!(
                    "timed out after {:?} waiting for machine to enter running state (current state: {})",
                    startup_timeout,
                    vm.state()
                )));
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                return Err(MachineError::Backend(
                    "machine state subscription disconnected while waiting for startup".to_string(),
                ));
            }
        };

        match event {
            VirtualMachineState::Stopped => {
                return Err(MachineError::Backend(
                    "machine stopped unexpectedly during startup".to_string(),
                ));
            }
            VirtualMachineState::Running => return Ok(vm),
            _ => continue,
        }
    }
}

pub(super) async unsafe fn stop_vm(vm: &VirtualMachine) -> Result<(), MachineError> {
    if vm.state() == VirtualMachineState::Stopped {
        return Ok(());
    }

    vm.stop().await?;
    let events = vm.subscribe_state();
    let shutdown_timeout = Duration::from_secs(60 * 5);

    loop {
        let event = match events.recv_timeout(shutdown_timeout) {
            Ok(event) => event,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                return Err(MachineError::Backend(format!(
                    "timed out after {:?} waiting for machine to stop (current state: {})",
                    shutdown_timeout,
                    vm.state()
                )));
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                return Err(MachineError::Backend(
                    "machine state subscription disconnected while waiting for shutdown"
                        .to_string(),
                ));
            }
        };

        match event {
            VirtualMachineState::Stopped => return Ok(()),
            VirtualMachineState::Error => {
                return Err(MachineError::Backend(
                    "machine entered error state while stopping".to_string(),
                ));
            }
            _ => continue,
        }
    }
}

pub(super) fn spawn_state_exit_watcher(
    events: Receiver<VirtualMachineState>,
    exit_sender: Arc<Mutex<Option<oneshot::Sender<MachineExitEvent>>>>,
) {
    thread::Builder::new()
        .name("vz-machine-state-watcher".to_string())
        .spawn(move || {
            while let Ok(state) = events.recv() {
                match state {
                    VirtualMachineState::Stopped => {
                        send_exit_once(&exit_sender, MachineState::Stopped, "machine stopped");
                        return;
                    }
                    VirtualMachineState::Error => {
                        send_exit_once(
                            &exit_sender,
                            MachineState::Stopped,
                            "machine entered error state",
                        );
                        return;
                    }
                    _ => {}
                }
            }
            send_exit_once(
                &exit_sender,
                MachineState::Stopped,
                "machine state watcher disconnected",
            );
        })
        .ok();
}

pub(super) fn send_exit_once(
    exit_sender: &Arc<Mutex<Option<oneshot::Sender<MachineExitEvent>>>>,
    state: MachineState,
    message: &str,
) {
    let sender = exit_sender.lock().ok().and_then(|mut guard| guard.take());
    if let Some(sender) = sender {
        let _ = sender.send(MachineExitEvent {
            state,
            message: message.to_string(),
        });
    }
}
