use std::env;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use bento_protocol::{DEFAULT_DISCOVERY_PORT, KERNEL_PARAM_DISCOVERY_PORT};

use crate::types::{MachineError, MachineKind, NetworkMode, ResolvedMachineSpec};

pub(super) const FIRECRACKER_BINARY_ENV: &str = "FIRECRACKER_BIN";
pub(super) const FIRECRACKER_BINARY_NAME: &str = "firecracker";
pub(super) const API_SOCKET_NAME: &str = "firecracker.sock";
pub(super) const TRACE_LOG_NAME: &str = "fc.trace.log";
pub(super) const VSOCK_SOCKET_NAME: &str = "firecracker.vsock";

pub(crate) fn validate(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    let config = &spec.config;
    if config.cpus.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires a CPU count".to_string(),
        });
    }

    if config.memory_mib.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires a memory size".to_string(),
        });
    }

    if config.kernel_path.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires a kernel image path".to_string(),
        });
    }

    if config.initramfs_path.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires an initramfs path".to_string(),
        });
    }

    if matches!(config.cpus, Some(0)) {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires at least one vCPU".to_string(),
        });
    }

    if matches!(config.memory_mib, Some(0)) {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires memory_mib to be greater than zero".to_string(),
        });
    }

    if !config.mounts.is_empty() {
        return not_implemented(
            spec,
            "shared directory mounts are not implemented for the firecracker backend yet",
        );
    }

    if config.machine_identifier_path.is_some() {
        return not_implemented(
            spec,
            "machine identifiers are not used by the firecracker backend",
        );
    }

    if config.nested_virtualization {
        return not_implemented(
            spec,
            "nested virtualization is not implemented for the firecracker backend yet",
        );
    }

    if config.rosetta {
        return not_implemented(
            spec,
            "rosetta is not implemented for the firecracker backend",
        );
    }

    match config.network {
        NetworkMode::None => {}
        NetworkMode::VzNat => {
            return not_implemented(spec, "vznat networking is only supported by the VZ backend");
        }
        NetworkMode::Bridged => {
            return not_implemented(
                spec,
                "bridged networking is not implemented for the firecracker backend yet",
            );
        }
        NetworkMode::Cni => {
            return not_implemented(
                spec,
                "cni networking is not implemented for the firecracker backend yet",
            );
        }
    }

    Ok(())
}

pub(crate) fn prepare(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    validate(spec)?;
    validate_support()?;

    let kernel_path = spec
        .config
        .kernel_path
        .as_ref()
        .expect("validated kernel path missing");
    let initramfs_path = spec
        .config
        .initramfs_path
        .as_ref()
        .expect("validated initramfs path missing");

    if spec.config.machine_directory.as_os_str().is_empty() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "machine_directory must be set".to_string(),
        });
    }

    ensure_path_exists(spec, kernel_path, "kernel image")?;
    ensure_path_exists(spec, initramfs_path, "initramfs")?;
    if let Some(root_disk) = spec.config.root_disk.as_ref() {
        ensure_path_exists(spec, &root_disk.path, "root disk")?;
    }
    for (index, disk) in spec.config.data_disks.iter().enumerate() {
        ensure_path_exists(spec, &disk.path, &format!("data disk #{index}"))?;
    }

    std::fs::create_dir_all(runtime_dir_for(spec))?;
    Ok(())
}

pub(super) fn validate_support() -> Result<(), MachineError> {
    locate_firecracker_binary()?;
    if !Path::new("/dev/kvm").exists() {
        return Err(MachineError::UnsupportedBackend {
            kind: MachineKind::Firecracker,
            reason: "firecracker requires /dev/kvm on Linux hosts".to_string(),
        });
    }

    Ok(())
}

pub(super) fn locate_firecracker_binary() -> Result<PathBuf, MachineError> {
    if let Some(path) = env::var_os(FIRECRACKER_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }

        return Err(MachineError::UnsupportedBackend {
            kind: MachineKind::Firecracker,
            reason: format!(
                "{FIRECRACKER_BINARY_ENV} is set but does not point to a file: {}",
                path.display()
            ),
        });
    }

    let path = env::var_os("PATH").ok_or_else(|| MachineError::UnsupportedBackend {
        kind: MachineKind::Firecracker,
        reason: "PATH is not set, so the firecracker binary cannot be located".to_string(),
    })?;

    for entry in env::split_paths(&path) {
        let candidate = entry.join(FIRECRACKER_BINARY_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(MachineError::UnsupportedBackend {
        kind: MachineKind::Firecracker,
        reason: "firecracker binary was not found in PATH".to_string(),
    })
}

pub(super) fn runtime_dir_for(spec: &ResolvedMachineSpec) -> PathBuf {
    spec.config.machine_directory.clone()
}

pub(super) fn ensure_path_exists(
    spec: &ResolvedMachineSpec,
    path: &Path,
    label: &str,
) -> Result<(), MachineError> {
    if path.exists() {
        return Ok(());
    }

    Err(MachineError::InvalidConfig {
        id: spec.id.clone(),
        reason: format!("{label} does not exist: {}", path.display()),
    })
}

fn not_implemented(spec: &ResolvedMachineSpec, reason: &str) -> Result<(), MachineError> {
    Err(MachineError::InvalidConfig {
        id: spec.id.clone(),
        reason: reason.to_string(),
    })
}

pub(super) fn build_boot_args(config: &crate::types::MachineConfig) -> String {
    let mut args = vec![
        "console=ttyS0".to_string(),
        "reboot=k".to_string(),
        "panic=1".to_string(),
        "pci=off".to_string(),
    ];
    if config.root_disk.is_some() {
        args.push("root=/dev/vda".to_string());
    }
    args.push(format!(
        "{}={}",
        KERNEL_PARAM_DISCOVERY_PORT, DEFAULT_DISCOVERY_PORT
    ));
    args.join(" ")
}

pub(super) fn guest_cid_for(spec: &ResolvedMachineSpec) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.id.as_str().hash(&mut hasher);
    let cid_base = 3u32;
    cid_base + (hasher.finish() as u32 % 0x3fff_fffc)
}
