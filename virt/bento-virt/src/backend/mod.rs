#[cfg(target_os = "linux")]
mod krun;
#[cfg(target_os = "macos")]
mod vz;

use std::sync::Arc;

#[cfg(target_os = "linux")]
pub(crate) type VmBackend = krun::KrunMachineBackend;
#[cfg(target_os = "macos")]
pub(crate) type VmBackend = vz::VzMachineBackend;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[derive(Debug)]
pub(crate) struct VmBackend;

use crate::types::{VmConfig, VmmError};

#[cfg(target_os = "macos")]
pub(crate) fn validate(config: &VmConfig) -> Result<(), VmmError> {
    vz::validate(config)
}

#[cfg(target_os = "linux")]
pub(crate) fn validate(config: &VmConfig) -> Result<(), VmmError> {
    krun::validate(config)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn validate(_config: &VmConfig) -> Result<(), VmmError> {
    Err(VmmError::UnsupportedBackend {
        kind: "none",
        reason: "no machine backend is available for this host platform".to_string(),
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn create_backend(config: VmConfig) -> Result<Arc<VmBackend>, VmmError> {
    Ok(Arc::new(vz::VzMachineBackend::new(config)?))
}

#[cfg(target_os = "linux")]
pub(crate) fn create_backend(config: VmConfig) -> Result<Arc<VmBackend>, VmmError> {
    Ok(Arc::new(krun::KrunMachineBackend::new(config)?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn create_backend(_config: VmConfig) -> Result<Arc<VmBackend>, VmmError> {
    Err(VmmError::UnsupportedBackend {
        kind: "none",
        reason: "no machine backend is available for this host platform".to_string(),
    })
}
