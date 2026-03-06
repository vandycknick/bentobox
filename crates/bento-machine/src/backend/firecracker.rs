use crate::backend::MachineBackend;
use crate::types::{
    MachineError, MachineKind, MachineState, OpenDeviceRequest, OpenDeviceResponse,
    ResolvedMachineSpec,
};
use std::sync::Mutex;

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

impl MachineBackend for FirecrackerMachineBackend {
    fn state(&self) -> Result<MachineState, MachineError> {
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)
    }

    fn start(&mut self) -> Result<(), MachineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| MachineError::RegistryPoisoned)?;
        *state = MachineState::Running;
        Ok(())
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
