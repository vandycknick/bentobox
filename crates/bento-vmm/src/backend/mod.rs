#[cfg(target_os = "linux")]
mod cloud_hypervisor;
#[cfg(target_os = "linux")]
mod firecracker;
#[cfg(target_os = "macos")]
mod vz;

use crate::stream::{MachineSerialStream, VsockStream};
use crate::types::{Backend, VmConfig, VmExit, VmmError};

#[derive(Debug)]
pub(crate) enum VmBackend {
    #[cfg(target_os = "linux")]
    CloudHypervisor(cloud_hypervisor::CloudHypervisorMachineBackend),
    #[cfg(target_os = "linux")]
    Firecracker(firecracker::FirecrackerMachineBackend),
    #[cfg(target_os = "macos")]
    Vz(vz::VzMachineBackend),
}

impl VmBackend {
    pub(crate) async fn start(&self) -> Result<(), VmmError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::CloudHypervisor(backend) => backend.start().await,
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.start().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.start().await,
        }
    }

    pub(crate) async fn stop(&self) -> Result<(), VmmError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::CloudHypervisor(backend) => backend.stop().await,
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.stop().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.stop().await,
        }
    }

    pub(crate) async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmmError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::CloudHypervisor(backend) => backend.connect_vsock(port).await,
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.connect_vsock(port).await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.connect_vsock(port).await,
        }
    }

    pub(crate) async fn open_serial(&self) -> Result<MachineSerialStream, VmmError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::CloudHypervisor(backend) => backend.open_serial().await,
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.open_serial().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.open_serial().await,
        }
    }

    pub(crate) async fn wait(&self) -> Result<VmExit, VmmError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::CloudHypervisor(backend) => backend.wait().await,
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.wait().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.wait().await,
        }
    }

    pub(crate) async fn try_wait(&self) -> Result<Option<VmExit>, VmmError> {
        match self {
            #[cfg(target_os = "linux")]
            Self::CloudHypervisor(backend) => backend.try_wait().await,
            #[cfg(target_os = "linux")]
            Self::Firecracker(backend) => backend.try_wait().await,
            #[cfg(target_os = "macos")]
            Self::Vz(backend) => backend.try_wait().await,
        }
    }
}

pub(crate) fn validate(backend: Backend, config: &VmConfig) -> Result<(), VmmError> {
    match backend {
        #[cfg(target_os = "macos")]
        Backend::Vz => vz::validate(config),
        #[cfg(target_os = "linux")]
        Backend::CloudHypervisor => cloud_hypervisor::validate(config),
        #[cfg(target_os = "linux")]
        Backend::Firecracker => firecracker::validate(config),
        kind => Err(VmmError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}

pub(crate) fn create_backend(backend: Backend, config: VmConfig) -> Result<VmBackend, VmmError> {
    match backend {
        #[cfg(target_os = "macos")]
        Backend::Vz => Ok(VmBackend::Vz(vz::VzMachineBackend::new(config)?)),
        #[cfg(target_os = "linux")]
        Backend::CloudHypervisor => Ok(VmBackend::CloudHypervisor(
            cloud_hypervisor::CloudHypervisorMachineBackend::new(config)?,
        )),
        #[cfg(target_os = "linux")]
        Backend::Firecracker => Ok(VmBackend::Firecracker(
            firecracker::FirecrackerMachineBackend::new(config)?,
        )),
        kind => Err(VmmError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}
