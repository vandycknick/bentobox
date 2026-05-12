#[cfg(target_os = "linux")]
mod firecracker;
#[cfg(target_os = "linux")]
mod krun;
#[cfg(target_os = "macos")]
mod vz;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::stream::{MachineSerialStream, VsockListener, VsockStream};
use crate::types::{Backend, VmConfig, VmExit, VmmError};

type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) type VmBackend = dyn MachineBackend;

pub(crate) trait MachineBackend: std::fmt::Debug + Send + Sync {
    fn kind(&self) -> Backend;

    fn start(&self) -> BackendFuture<'_, Result<(), VmmError>>;

    fn stop(&self) -> BackendFuture<'_, Result<(), VmmError>>;

    fn connect_vsock(&self, port: u32) -> BackendFuture<'_, Result<VsockStream, VmmError>>;

    fn listen_vsock(&self, _port: u32) -> BackendFuture<'_, Result<VsockListener, VmmError>> {
        Box::pin(async move {
            Err(VmmError::Unimplemented {
                kind: self.kind(),
                operation: "listen_vsock",
            })
        })
    }

    fn open_serial(&self) -> BackendFuture<'_, Result<MachineSerialStream, VmmError>>;

    fn wait(&self) -> BackendFuture<'_, Result<VmExit, VmmError>>;

    fn try_wait(&self) -> BackendFuture<'_, Result<Option<VmExit>, VmmError>>;
}

#[cfg(target_os = "linux")]
impl MachineBackend for krun::KrunMachineBackend {
    fn kind(&self) -> Backend {
        Backend::Krun
    }

    fn start(&self) -> BackendFuture<'_, Result<(), VmmError>> {
        Box::pin(krun::KrunMachineBackend::start(self))
    }

    fn stop(&self) -> BackendFuture<'_, Result<(), VmmError>> {
        Box::pin(krun::KrunMachineBackend::stop(self))
    }

    fn connect_vsock(&self, port: u32) -> BackendFuture<'_, Result<VsockStream, VmmError>> {
        Box::pin(krun::KrunMachineBackend::connect_vsock(self, port))
    }

    fn listen_vsock(&self, port: u32) -> BackendFuture<'_, Result<VsockListener, VmmError>> {
        Box::pin(krun::KrunMachineBackend::listen_vsock(self, port))
    }

    fn open_serial(&self) -> BackendFuture<'_, Result<MachineSerialStream, VmmError>> {
        Box::pin(krun::KrunMachineBackend::open_serial(self))
    }

    fn wait(&self) -> BackendFuture<'_, Result<VmExit, VmmError>> {
        Box::pin(krun::KrunMachineBackend::wait(self))
    }

    fn try_wait(&self) -> BackendFuture<'_, Result<Option<VmExit>, VmmError>> {
        Box::pin(krun::KrunMachineBackend::try_wait(self))
    }
}

#[cfg(target_os = "linux")]
impl MachineBackend for firecracker::FirecrackerMachineBackend {
    fn kind(&self) -> Backend {
        Backend::Firecracker
    }

    fn start(&self) -> BackendFuture<'_, Result<(), VmmError>> {
        Box::pin(firecracker::FirecrackerMachineBackend::start(self))
    }

    fn stop(&self) -> BackendFuture<'_, Result<(), VmmError>> {
        Box::pin(firecracker::FirecrackerMachineBackend::stop(self))
    }

    fn connect_vsock(&self, port: u32) -> BackendFuture<'_, Result<VsockStream, VmmError>> {
        Box::pin(firecracker::FirecrackerMachineBackend::connect_vsock(
            self, port,
        ))
    }

    fn open_serial(&self) -> BackendFuture<'_, Result<MachineSerialStream, VmmError>> {
        Box::pin(firecracker::FirecrackerMachineBackend::open_serial(self))
    }

    fn wait(&self) -> BackendFuture<'_, Result<VmExit, VmmError>> {
        Box::pin(firecracker::FirecrackerMachineBackend::wait(self))
    }

    fn try_wait(&self) -> BackendFuture<'_, Result<Option<VmExit>, VmmError>> {
        Box::pin(firecracker::FirecrackerMachineBackend::try_wait(self))
    }
}

#[cfg(target_os = "macos")]
impl MachineBackend for vz::VzMachineBackend {
    fn kind(&self) -> Backend {
        Backend::Vz
    }

    fn start(&self) -> BackendFuture<'_, Result<(), VmmError>> {
        Box::pin(vz::VzMachineBackend::start(self))
    }

    fn stop(&self) -> BackendFuture<'_, Result<(), VmmError>> {
        Box::pin(vz::VzMachineBackend::stop(self))
    }

    fn connect_vsock(&self, port: u32) -> BackendFuture<'_, Result<VsockStream, VmmError>> {
        Box::pin(vz::VzMachineBackend::connect_vsock(self, port))
    }

    fn listen_vsock(&self, port: u32) -> BackendFuture<'_, Result<VsockListener, VmmError>> {
        Box::pin(vz::VzMachineBackend::listen_vsock(self, port))
    }

    fn open_serial(&self) -> BackendFuture<'_, Result<MachineSerialStream, VmmError>> {
        Box::pin(vz::VzMachineBackend::open_serial(self))
    }

    fn wait(&self) -> BackendFuture<'_, Result<VmExit, VmmError>> {
        Box::pin(vz::VzMachineBackend::wait(self))
    }

    fn try_wait(&self) -> BackendFuture<'_, Result<Option<VmExit>, VmmError>> {
        Box::pin(vz::VzMachineBackend::try_wait(self))
    }
}

pub(crate) fn validate(backend: Backend, config: &VmConfig) -> Result<(), VmmError> {
    match backend {
        #[cfg(target_os = "macos")]
        Backend::Vz => vz::validate(config),
        #[cfg(target_os = "linux")]
        Backend::Krun => krun::validate(config),
        #[cfg(target_os = "linux")]
        Backend::Firecracker => firecracker::validate(config),
        kind => Err(VmmError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}

pub(crate) fn create_backend(
    backend: Backend,
    config: VmConfig,
) -> Result<Arc<VmBackend>, VmmError> {
    match backend {
        #[cfg(target_os = "macos")]
        Backend::Vz => Ok(Arc::new(vz::VzMachineBackend::new(config)?)),
        #[cfg(target_os = "linux")]
        Backend::Krun => Ok(Arc::new(krun::KrunMachineBackend::new(config)?)),
        #[cfg(target_os = "linux")]
        Backend::Firecracker => Ok(Arc::new(firecracker::FirecrackerMachineBackend::new(
            config,
        )?)),
        kind => Err(VmmError::UnsupportedBackend {
            kind,
            reason: "backend is not compiled for this host platform".to_string(),
        }),
    }
}
