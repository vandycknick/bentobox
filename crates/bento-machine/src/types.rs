use std::os::fd::OwnedFd;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MachineId(String);

impl MachineId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for MachineId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for MachineId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MachineKind {
    Vz,
    Firecracker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineConfig {
    pub cpus: Option<usize>,
    pub memory_mib: Option<u64>,
    pub kernel_path: Option<PathBuf>,
    pub initramfs_path: Option<PathBuf>,
    pub machine_identifier_path: Option<PathBuf>,
    pub nested_virtualization: bool,
    pub network: NetworkMode,
    pub root_disk: Option<DiskImage>,
    pub data_disks: Vec<DiskImage>,
    pub mounts: Vec<SharedDirectory>,
}

impl MachineConfig {
    pub fn new() -> Self {
        Self {
            cpus: None,
            memory_mib: None,
            kernel_path: None,
            initramfs_path: None,
            machine_identifier_path: None,
            nested_virtualization: false,
            network: NetworkMode::None,
            root_disk: None,
            data_disks: Vec::new(),
            mounts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskImage {
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDirectory {
    pub host_path: PathBuf,
    pub tag: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    VzNat,
    None,
    Bridged,
    Cni,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineSpec {
    pub id: MachineId,
    pub kind: Option<MachineKind>,
    pub config: MachineConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMachineSpec {
    pub id: MachineId,
    pub kind: MachineKind,
    pub config: MachineConfig,
}

impl MachineSpec {
    pub(crate) fn resolve(self) -> Result<ResolvedMachineSpec, MachineError> {
        Ok(ResolvedMachineSpec {
            id: self.id,
            kind: resolve_machine_kind(self.kind)?,
            config: self.config,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineState {
    Created,
    Running,
    Stopped,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDeviceRequest {
    Vsock { port: u32 },
    Serial,
}

#[derive(Debug)]
pub enum OpenDeviceResponse {
    Vsock {
        stream: OwnedFd,
    },
    Serial {
        guest_input: OwnedFd,
        guest_output: OwnedFd,
    },
}

#[derive(Debug, Error)]
pub enum MachineError {
    #[error("machine {id:?} was requested with a different spec")]
    SpecMismatch {
        id: MachineId,
        existing: Box<ResolvedMachineSpec>,
        requested: Box<ResolvedMachineSpec>,
    },

    #[error("machine {id:?} has been released")]
    MachineReleased { id: MachineId },

    #[error("machine backend {kind:?} is unsupported on this host: {reason}")]
    UnsupportedBackend { kind: MachineKind, reason: String },

    #[error("machine backend {kind:?} does not implement {operation} yet")]
    Unimplemented {
        kind: MachineKind,
        operation: &'static str,
    },

    #[error("machine {id:?} is invalid: {reason}")]
    InvalidConfig { id: MachineId, reason: String },

    #[error("backend error: {0}")]
    Backend(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("machine registry lock was poisoned")]
    RegistryPoisoned,
}

fn resolve_machine_kind(kind: Option<MachineKind>) -> Result<MachineKind, MachineError> {
    match kind {
        Some(kind) => Ok(kind),
        None => auto_machine_kind(),
    }
}

#[cfg(target_os = "macos")]
fn auto_machine_kind() -> Result<MachineKind, MachineError> {
    Ok(MachineKind::Vz)
}

#[cfg(target_os = "linux")]
fn auto_machine_kind() -> Result<MachineKind, MachineError> {
    Ok(MachineKind::Firecracker)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn auto_machine_kind() -> Result<MachineKind, MachineError> {
    Err(MachineError::UnsupportedBackend {
        kind: MachineKind::Vz,
        reason: "no machine backend is available for this host platform".to_string(),
    })
}
