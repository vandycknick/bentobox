#[cfg(target_os = "linux")]
mod firecracker;
#[cfg(target_os = "macos")]
mod vz;

use crate::types::{
    MachineError, MachineKind, MachineState, OpenDeviceRequest, OpenDeviceResponse,
    ResolvedMachineSpec,
};

pub(crate) trait MachineBackend {
    fn state(&self) -> Result<MachineState, MachineError>;
    fn start(&mut self) -> Result<(), MachineError>;
    fn stop(&mut self) -> Result<(), MachineError>;
    fn open_device(&self, request: OpenDeviceRequest) -> Result<OpenDeviceResponse, MachineError>;
}

pub(crate) fn create_backend(
    spec: &ResolvedMachineSpec,
) -> Result<Box<dyn MachineBackend>, MachineError> {
    match spec.kind {
        #[cfg(target_os = "macos")]
        MachineKind::Vz => Ok(Box::new(vz::VzMachineBackend::new(spec.clone())?)),
        #[cfg(target_os = "linux")]
        MachineKind::Firecracker => Ok(Box::new(firecracker::FirecrackerMachineBackend::new(
            spec.clone(),
        )?)),
        kind => Err(MachineError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}
