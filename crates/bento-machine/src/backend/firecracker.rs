use crate::backend::MachineBackend;
use crate::types::{
    MachineError, MachineExitEvent, MachineExitReceiver, MachineKind, MachineState,
    OpenDeviceRequest, OpenDeviceResponse, ResolvedMachineSpec,
};
use std::sync::Mutex;
use tokio::sync::oneshot;

#[derive(Debug)]
pub(crate) struct FirecrackerMachineBackend {
    _spec: ResolvedMachineSpec,
    state: Mutex<MachineState>,
}

impl FirecrackerMachineBackend {
    pub(crate) fn new(spec: ResolvedMachineSpec) -> Result<Self, MachineError> {
        Ok(Self {
            _spec: spec,
            state: Mutex::new(MachineState::Created),
        })
    }
}

pub(crate) fn validate(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    Err(MachineError::UnsupportedBackend {
        kind: spec.kind,
        reason: "firecracker backend is not implemented yet".to_string(),
    })
}

pub(crate) fn prepare(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    Err(MachineError::UnsupportedBackend {
        kind: spec.kind,
        reason: "firecracker backend is not implemented yet".to_string(),
    })
}

impl MachineBackend for FirecrackerMachineBackend {
    fn state(&self) -> Result<MachineState, MachineError> {
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)
    }

    fn start(&mut self) -> Result<MachineExitReceiver, MachineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| MachineError::RegistryPoisoned)?;
        if *state == MachineState::Running {
            return Err(MachineError::AlreadyRunning {
                id: self._spec.id.clone(),
            });
        }
        *state = MachineState::Running;
        let (_tx, rx) = oneshot::channel::<MachineExitEvent>();
        Ok(rx)
    }

    fn stop(&mut self) -> Result<(), MachineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| MachineError::RegistryPoisoned)?;
        *state = MachineState::Stopped;
        Ok(())
    }

    fn open_device(&self, _request: OpenDeviceRequest) -> Result<OpenDeviceResponse, MachineError> {
        Err(MachineError::Unimplemented {
            kind: MachineKind::Firecracker,
            operation: "open_device",
        })
    }
}
