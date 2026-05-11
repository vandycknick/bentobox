use std::path::PathBuf;

use crate::error::{KrunBackendError, Result};

pub const DEFAULT_ID: &str = "anonymous-instance";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KrunConfig {
    pub id: String,
    pub cpus: u8,
    pub memory_mib: u32,
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    pub cmdline: Vec<String>,
    pub disks: Vec<Disk>,
    pub mounts: Vec<Mount>,
    pub vsock_ports: Vec<VsockPort>,
    pub net_unixgrams: Vec<NetUnixgram>,
    pub stdio_console: bool,
    pub disable_implicit_vsock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub block_id: String,
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub tag: String,
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VsockPort {
    pub port: u32,
    pub path: PathBuf,
    pub listen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetUnixgram {
    pub peer_path: PathBuf,
    pub mac: [u8; 6],
}

impl Default for KrunConfig {
    fn default() -> Self {
        Self {
            id: DEFAULT_ID.to_string(),
            cpus: 1,
            memory_mib: 512,
            kernel: None,
            initramfs: None,
            cmdline: Vec::new(),
            disks: Vec::new(),
            mounts: Vec::new(),
            vsock_ports: Vec::new(),
            net_unixgrams: Vec::new(),
            stdio_console: false,
            disable_implicit_vsock: false,
        }
    }
}

pub fn validate_config(config: &KrunConfig) -> Result<()> {
    if config.cpus == 0 {
        return Err(KrunBackendError::InvalidConfig(
            "krun requires at least one vCPU".to_string(),
        ));
    }
    if config.memory_mib == 0 {
        return Err(KrunBackendError::InvalidConfig(
            "krun requires memory_mib to be greater than zero".to_string(),
        ));
    }
    if config.kernel.is_none() {
        return Err(KrunBackendError::InvalidConfig(
            "krun requires a kernel".to_string(),
        ));
    }
    if !config.net_unixgrams.is_empty() && config.id.is_empty() {
        return Err(KrunBackendError::InvalidConfig(
            "net unixgram requires a non-empty VM id".to_string(),
        ));
    }
    for net in &config.net_unixgrams {
        if net.peer_path.as_os_str().is_empty() {
            return Err(KrunBackendError::InvalidConfig(
                "net unixgram peer path cannot be empty".to_string(),
            ));
        }
        if net.mac[0] & 0x01 != 0 {
            return Err(KrunBackendError::InvalidConfig(
                "net unixgram mac cannot be multicast".to_string(),
            ));
        }
    }
    Ok(())
}
