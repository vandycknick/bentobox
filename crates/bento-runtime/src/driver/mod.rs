use std::io;
use std::os::fd::OwnedFd;

use thiserror::Error;

use crate::instance::Instance;

#[cfg(target_os = "macos")]
pub mod vz;

#[cfg(target_os = "linux")]
pub mod firecracker;

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("backend error: {0}")]
    Backend(String),

    // TODO:I don't want to leak this
    #[cfg(target_os = "macos")]
    #[error(transparent)]
    VirtualMachine(#[from] crate::driver::vz::VirtualMachineError),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    InstanceDisk(#[from] crate::instance::InstanceDiskError),

    #[error(transparent)]
    InstanceBoot(#[from] crate::instance::InstanceBootError),
}

pub trait Driver {
    fn validate(&self) -> Result<(), DriverError>;

    fn create(&self) -> Result<(), DriverError>;

    fn start(&mut self) -> Result<(), DriverError>;

    fn stop(&mut self) -> Result<(), DriverError>;

    fn open_vsock_stream(&self, _port: u32) -> Result<OwnedFd, DriverError> {
        Err(DriverError::Backend(
            "driver does not support opening vsock streams".to_string(),
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn get_driver_for(inst: &Instance) -> Result<Box<dyn Driver>, DriverError> {
    use crate::instance::EngineType;

    match inst.engine() {
        EngineType::VZ => Ok(Box::new(vz::VzDriver::new(inst.clone()))),
    }
}

#[cfg(target_os = "linux")]
pub fn get_driver_for(inst: &Instance) -> Result<Box<dyn Driver>, DriverError> {
    match inst.engine() {
        crate::instance::EngineType::VZ => Err(DriverError::Backend(
            "VZ driver is only supported on macOS hosts".to_string(),
        )),
    }
}
