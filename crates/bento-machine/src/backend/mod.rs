#[cfg(target_os = "linux")]
mod firecracker;
#[cfg(target_os = "macos")]
mod vz;

use crate::stream::{RawSerialConnection, RawVsockConnection};
use crate::types::{
    MachineError, MachineExitReceiver, MachineKind, MachineState, ResolvedMachineSpec,
};

#[derive(Debug)]
pub(crate) enum Backend {
    #[cfg(target_os = "linux")]
    Firecracker(firecracker::FirecrackerMachineBackend),
    #[cfg(target_os = "macos")]
    Vz(vz::VzMachineBackend),
}

impl Backend {
    pub(crate) async fn state(&self) -> Result<MachineState, MachineError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.state().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.state().await,
        }
    }

    pub(crate) async fn start(&self) -> Result<MachineExitReceiver, MachineError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.start().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.start().await,
        }
    }

    pub(crate) async fn stop(&self) -> Result<(), MachineError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.stop().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.stop().await,
        }
    }

    pub(crate) async fn open_vsock(&self, port: u32) -> Result<RawVsockConnection, MachineError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.open_vsock(port).await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.open_vsock(port).await,
        }
    }

    pub(crate) async fn open_serial(&self) -> Result<RawSerialConnection, MachineError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.open_serial().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.open_serial().await,
        }
    }
}

pub(crate) fn validate(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    match spec.kind {
        #[cfg(target_os = "macos")]
        MachineKind::Vz => vz::validate(spec),
        #[cfg(target_os = "linux")]
        MachineKind::Firecracker => firecracker::validate(spec),
        kind => Err(MachineError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}

pub(crate) fn prepare(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    match spec.kind {
        #[cfg(target_os = "macos")]
        MachineKind::Vz => vz::prepare(spec),
        #[cfg(target_os = "linux")]
        MachineKind::Firecracker => firecracker::prepare(spec),
        kind => Err(MachineError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}

pub(crate) fn create_backend(spec: &ResolvedMachineSpec) -> Result<Backend, MachineError> {
    match spec.kind {
        #[cfg(target_os = "macos")]
        MachineKind::Vz => Ok(Backend::Vz(vz::VzMachineBackend::new(spec.clone())?)),
        #[cfg(target_os = "linux")]
        MachineKind::Firecracker => Ok(Backend::Firecracker(
            firecracker::FirecrackerMachineBackend::new(spec.clone())?,
        )),
        kind => Err(MachineError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}
