use std::sync::{Arc, Mutex};

use crate::stream::{RawSerialConnection, RawVsockConnection};
use crate::types::{
    MachineError, MachineExitEvent, MachineExitReceiver, MachineState, ResolvedMachineSpec,
};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

use crate::backend::vz::config::{create_vm_config, validate_support};
use crate::backend::vz::lifecycle::{
    build_vm, send_exit_once, spawn_state_exit_watcher, start_vm, stop_vm,
};
use crate::backend::vz::vm::VirtualMachine;

#[derive(Debug)]
pub(crate) struct VzMachineBackend {
    spec: ResolvedMachineSpec,
    inner: AsyncMutex<VzMachineState>,
}

#[derive(Debug)]
struct VzMachineState {
    vm: Option<VirtualMachine>,
    state: MachineState,
    exit_sender: Option<Arc<Mutex<Option<oneshot::Sender<MachineExitEvent>>>>>,
}

impl VzMachineBackend {
    pub(crate) fn new(spec: ResolvedMachineSpec) -> Result<Self, MachineError> {
        super::validate(&spec)?;
        Ok(Self {
            spec,
            inner: AsyncMutex::new(VzMachineState {
                vm: None,
                state: MachineState::Created,
                exit_sender: None,
            }),
        })
    }

    pub(crate) async fn state(&self) -> Result<MachineState, MachineError> {
        let state = self.inner.lock().await;
        Ok(match state.vm.as_ref() {
            Some(vm) => vm.state().into(),
            None => state.state,
        })
    }

    pub(crate) async fn start(&self) -> Result<MachineExitReceiver, MachineError> {
        validate_support()?;
        let mut state = self.inner.lock().await;
        if state.vm.is_some() {
            return Err(MachineError::AlreadyRunning {
                id: self.spec.id.clone(),
            });
        }

        let (exit_tx, exit_rx) = oneshot::channel();
        let shared_exit = Arc::new(Mutex::new(Some(exit_tx)));

        unsafe {
            let config = create_vm_config(&self.spec)?;
            let vm = build_vm(config);
            let vm = start_vm(vm).await?;
            spawn_state_exit_watcher(vm.subscribe_state(), shared_exit.clone());
            state.vm = Some(vm);
        }

        state.state = MachineState::Running;
        state.exit_sender = Some(shared_exit);
        Ok(exit_rx)
    }

    pub(crate) async fn stop(&self) -> Result<(), MachineError> {
        let mut state = self.inner.lock().await;
        if let Some(vm) = state.vm.as_ref() {
            unsafe {
                stop_vm(vm).await?;
            }
        }

        if let Some(exit_sender) = state.exit_sender.take() {
            send_exit_once(
                &exit_sender,
                MachineState::Stopped,
                "machine stopped by host request",
            );
        }

        state.vm = None;
        state.state = MachineState::Stopped;
        Ok(())
    }

    pub(crate) async fn open_vsock(&self, port: u32) -> Result<RawVsockConnection, MachineError> {
        let vm = {
            let state = self.inner.lock().await;
            state.vm.clone().ok_or_else(|| {
                MachineError::Backend(format!(
                    "cannot open vsock stream because machine {:?} is not running",
                    self.spec.id.as_str()
                ))
            })?
        };

        vm.open_vsock_stream(port)
            .await
            .map(RawVsockConnection::File)
            .map_err(MachineError::from)
    }

    pub(crate) async fn open_serial(&self) -> Result<RawSerialConnection, MachineError> {
        let vm = {
            let state = self.inner.lock().await;
            state.vm.clone().ok_or_else(|| {
                MachineError::Backend(format!(
                    "cannot open serial stream because machine {:?} is not running",
                    self.spec.id.as_str()
                ))
            })?
        };

        vm.open_serial_files()
            .map_err(MachineError::from)
            .map(|(read, write)| RawSerialConnection { read, write })
    }
}
