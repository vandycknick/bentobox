use std::path::{Path, PathBuf};

use bento_core::{
    resolve_mount_location, Backend as SpecBackend, DiskKind, InstanceFile,
    NetworkMode as SpecNetworkMode, VmSpec,
};
use bento_vmm::{
    Backend, DiskImage, MachineIdentifier, NetworkMode, SharedDirectory, VmConfig, VmmError,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MachineSpecError {
    #[error(transparent)]
    Machine(#[from] VmmError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("invalid mount location: {0}")]
    InvalidMountLocation(String),

    #[error("invalid mount tag for {mount_source}: mount tags must be non-empty")]
    InvalidMountTag { mount_source: String },
}

#[derive(Debug, Clone)]
pub(crate) struct InstanceVmConfig {
    pub config: VmConfig,
    pub machine_identifier: Option<MachineIdentifier>,
}

pub(crate) struct VmSpecInputs<'a> {
    pub name: &'a str,
    pub data_dir: &'a Path,
    pub spec: &'a VmSpec,
}

pub(crate) fn vm_spec_machine_config(
    inputs: VmSpecInputs<'_>,
) -> Result<InstanceVmConfig, MachineSpecError> {
    let boot_assets = vm_spec_boot_assets(&inputs)?;
    let machine_identifier = if backend_needs_machine_identifier(inputs.spec.platform.backend) {
        Some(load_machine_identifier_from_dir(inputs.data_dir)?)
    } else {
        None
    };

    let mut builder = VmConfig::builder(inputs.name)
        .base_directory(inputs.data_dir.to_path_buf())
        .network(map_vm_spec_network_mode(inputs.spec.network.mode))
        .nested_virtualization(inputs.spec.settings.nested_virtualization)
        .rosetta(inputs.spec.settings.rosetta);

    if let Some(machine_identifier) = machine_identifier.clone() {
        builder = builder.machine_identifier(machine_identifier);
    }

    builder = builder
        .cpus(inputs.spec.resources.cpus as usize)
        .memory(inputs.spec.resources.memory_mib as u64)
        .kernel(boot_assets.kernel)
        .initramfs(boot_assets.initramfs);

    for disk in &inputs.spec.storage.disks {
        let disk_image = DiskImage {
            path: resolve_spec_path(inputs.data_dir, &disk.path),
            read_only: disk.read_only,
        };

        match disk.kind {
            DiskKind::Root => builder = builder.root_disk(disk_image),
            DiskKind::Data | DiskKind::Seed => builder = builder.disk(disk_image),
        }
    }

    let cidata_path = inputs.data_dir.join(InstanceFile::CidataDisk.as_str());
    if cidata_path.is_file() {
        builder = builder.disk(DiskImage {
            path: cidata_path,
            read_only: true,
        });
    }

    for mount in &inputs.spec.mounts {
        let host_path = resolve_mount_location(&mount.source)
            .map_err(MachineSpecError::InvalidMountLocation)?;
        if mount.tag.trim().is_empty() {
            return Err(MachineSpecError::InvalidMountTag {
                mount_source: mount.source.display().to_string(),
            });
        }
        builder = builder.mount(SharedDirectory {
            host_path,
            tag: mount.tag.clone(),
            read_only: mount.read_only,
        });
    }

    Ok(InstanceVmConfig {
        config: builder.build(),
        machine_identifier,
    })
}

pub(crate) fn machine_identifier_path_from_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(InstanceFile::AppleMachineIdentifier.as_str())
}

fn load_machine_identifier_from_dir(
    data_dir: &Path,
) -> Result<MachineIdentifier, MachineSpecError> {
    let path = machine_identifier_path_from_dir(data_dir);
    match std::fs::read(path) {
        Ok(bytes) => Ok(MachineIdentifier::from_bytes(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(MachineIdentifier::new()),
        Err(err) => Err(err.into()),
    }
}

struct BootAssets {
    kernel: PathBuf,
    initramfs: PathBuf,
}

fn vm_spec_boot_assets(inputs: &VmSpecInputs<'_>) -> Result<BootAssets, MachineSpecError> {
    let default_root = || -> Result<PathBuf, std::io::Error> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".local/share")));

        data_home
            .map(|d| d.join("bento/kernels/default"))
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "default kernel bundle root unavailable",
                )
            })
    };

    let kernel = match inputs.spec.boot.kernel.as_ref() {
        Some(path) => resolve_spec_path(inputs.data_dir, path),
        None => default_root()?.join("kernel"),
    };
    let initramfs = match inputs.spec.boot.initramfs.as_ref() {
        Some(path) => resolve_spec_path(inputs.data_dir, path),
        None => default_root()?.join("initramfs"),
    };

    if !kernel.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("kernel is not a file: {}", kernel.display()),
        )
        .into());
    }

    if !initramfs.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("initramfs is not a file: {}", initramfs.display()),
        )
        .into());
    }

    Ok(BootAssets { kernel, initramfs })
}

fn resolve_spec_path(data_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        data_dir.join(path)
    }
}

fn backend_needs_machine_identifier(backend: SpecBackend) -> bool {
    matches!(backend, SpecBackend::Auto | SpecBackend::Vz)
}

pub(crate) fn machine_backend_from_vm_spec(spec: &VmSpec) -> Result<Backend, VmmError> {
    match spec.platform.backend {
        SpecBackend::Auto | SpecBackend::Vz => Ok(Backend::Vz),
        SpecBackend::Firecracker => Ok(Backend::Firecracker),
        SpecBackend::CloudHypervisor => Ok(Backend::CloudHypervisor),
    }
}

fn map_vm_spec_network_mode(mode: SpecNetworkMode) -> NetworkMode {
    match mode {
        SpecNetworkMode::None => NetworkMode::None,
        SpecNetworkMode::User => NetworkMode::VzNat,
        SpecNetworkMode::Bridged => NetworkMode::Bridged,
    }
}

#[cfg(test)]
mod tests {
    use super::machine_backend_from_vm_spec;
    use bento_core::{
        Architecture, Backend as SpecBackend, Boot, GuestOs, Network,
        NetworkMode as SpecNetworkMode, Platform, Resources, Settings, Storage, VmSpec,
    };
    use bento_vmm::Backend;

    #[test]
    fn machine_backend_maps_cloud_hypervisor_engine() {
        let spec = VmSpec {
            version: 1,
            name: "devbox".to_string(),
            platform: Platform {
                guest_os: GuestOs::Linux,
                architecture: Architecture::Aarch64,
                backend: SpecBackend::CloudHypervisor,
            },
            resources: Resources {
                cpus: 2,
                memory_mib: 1024,
            },
            boot: Boot {
                kernel: None,
                initramfs: None,
                kernel_cmdline: Vec::new(),
                bootstrap: None,
            },
            storage: Storage { disks: Vec::new() },
            mounts: Vec::new(),
            network: Network {
                mode: SpecNetworkMode::None,
            },
            settings: Settings {
                nested_virtualization: false,
                rosetta: false,
                guest_enabled: false,
            },
        };
        let backend = machine_backend_from_vm_spec(&spec).expect("backend should resolve");
        assert_eq!(backend, Backend::CloudHypervisor);
    }
}
