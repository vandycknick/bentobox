use std::path::{Path, PathBuf};

use bento_core::{Backend as SpecBackend, DiskKind, NetworkMode as SpecNetworkMode, VmSpec};
use bento_vmm::{
    Backend, DiskImage, MachineIdentifier, NetworkMode, SharedDirectory, VmConfig, VmmError,
};
use thiserror::Error;

use bento_runtime::directories::Directory;
use bento_runtime::instance::{
    resolve_mount_location, EngineType, Instance, InstanceFile, NetworkMode as InstanceNetworkMode,
};

#[derive(Debug, Error)]
pub enum MachineSpecError {
    #[error(transparent)]
    Machine(#[from] VmmError),

    #[error(transparent)]
    InstanceDisk(#[from] bento_runtime::instance::InstanceDiskError),

    #[error(transparent)]
    InstanceBoot(#[from] bento_runtime::instance::InstanceBootError),

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

pub fn prepare_instance(inst: &Instance) -> Result<(), MachineSpecError> {
    let _ = instance_machine_config(inst)?;
    Ok(())
}

pub(crate) fn instance_machine_config(
    inst: &Instance,
) -> Result<InstanceVmConfig, MachineSpecError> {
    let engine = inst.engine();
    let boot_assets = inst.boot_assets()?;
    let machine_identifier = match engine {
        EngineType::VZ => Some(load_machine_identifier(inst)?),
        EngineType::Firecracker => None,
        EngineType::CloudHypervisor => None,
    };

    let mut builder = VmConfig::builder(inst.name.as_str())
        .base_directory(inst.dir().to_path_buf())
        .network(map_network_mode(inst.resolved_network_mode()))
        .nested_virtualization(inst.config.nested_virtualization.unwrap_or(false))
        .rosetta(inst.config.rosetta.unwrap_or(false));

    if let Some(machine_identifier) = machine_identifier.clone() {
        builder = builder.machine_identifier(machine_identifier);
    }

    if let Some(cpus) = inst.config.cpus {
        builder = builder.cpus(cpus as usize);
    }

    if let Some(memory) = inst.config.memory {
        builder = builder.memory(memory as u64);
    }

    builder = builder
        .kernel(boot_assets.kernel)
        .initramfs(boot_assets.initramfs);

    if let Some(disk) = inst.root_disk()? {
        builder = builder.root_disk(DiskImage {
            path: disk.path,
            read_only: disk.read_only,
        });
    }

    for disk in inst.data_disks()? {
        builder = builder.disk(DiskImage {
            path: disk.path,
            read_only: disk.read_only,
        });
    }

    for (index, mount) in inst.config.mounts.iter().enumerate() {
        let host_path = resolve_mount_location(&mount.location)
            .map_err(MachineSpecError::InvalidMountLocation)?;
        builder = builder.mount(SharedDirectory {
            host_path,
            tag: format!("mount{index}"),
            read_only: !mount.writable,
        });
    }

    Ok(InstanceVmConfig {
        config: builder.build(),
        machine_identifier,
    })
}

pub(crate) fn vm_spec_machine_config(
    inputs: VmSpecInputs<'_>,
) -> Result<InstanceVmConfig, MachineSpecError> {
    let engine = backend_to_engine(inputs.spec.platform.backend);
    let boot_assets = vm_spec_boot_assets(&inputs)?;
    let machine_identifier = match engine {
        EngineType::VZ => Some(load_machine_identifier_from_dir(inputs.data_dir)?),
        EngineType::Firecracker => None,
        EngineType::CloudHypervisor => None,
    };

    let mut builder = VmConfig::builder(inputs.name)
        .base_directory(inputs.data_dir.to_path_buf())
        .network(map_vm_spec_network_mode(inputs.spec.network.mode))
        .nested_virtualization(inputs.spec.host.nested_virtualization)
        .rosetta(inputs.spec.host.rosetta);

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

pub(crate) fn machine_identifier_path(inst: &Instance) -> std::path::PathBuf {
    inst.file(InstanceFile::AppleMachineIdentifier)
}

pub(crate) fn machine_identifier_path_from_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(InstanceFile::AppleMachineIdentifier.as_str())
}

fn load_machine_identifier(inst: &Instance) -> Result<MachineIdentifier, MachineSpecError> {
    let path = machine_identifier_path(inst);
    match std::fs::read(path) {
        Ok(bytes) => Ok(MachineIdentifier::from_bytes(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(MachineIdentifier::new()),
        Err(err) => Err(err.into()),
    }
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

fn vm_spec_boot_assets(
    inputs: &VmSpecInputs<'_>,
) -> Result<bento_runtime::instance::BootAssets, MachineSpecError> {
    let default_root = || {
        Directory::with_prefix("kernels")
            .get_data_home()
            .ok_or(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "default kernel bundle root unavailable",
            ))
            .map(|path| path.join("default"))
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

    Ok(bento_runtime::instance::BootAssets { kernel, initramfs })
}

fn resolve_spec_path(data_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        data_dir.join(path)
    }
}

fn backend_to_engine(backend: SpecBackend) -> EngineType {
    match backend {
        SpecBackend::Auto | SpecBackend::Vz => EngineType::VZ,
        SpecBackend::Firecracker => EngineType::Firecracker,
        SpecBackend::CloudHypervisor => EngineType::CloudHypervisor,
    }
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

fn map_network_mode(mode: InstanceNetworkMode) -> NetworkMode {
    match mode {
        InstanceNetworkMode::VzNat => NetworkMode::VzNat,
        InstanceNetworkMode::None => NetworkMode::None,
        InstanceNetworkMode::Bridged => NetworkMode::Bridged,
        InstanceNetworkMode::Cni => NetworkMode::Cni,
    }
}

#[cfg(test)]
mod tests {
    use super::machine_backend_from_vm_spec;
    use bento_core::{
        Architecture, Backend as SpecBackend, Boot, Capabilities, Guest, GuestOs, Host, Network,
        NetworkMode as SpecNetworkMode, Platform, Resources, Storage, VmSpec,
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
            guest: Guest {
                profiles: Vec::new(),
                capabilities: Capabilities {
                    ssh: false,
                    docker: false,
                    dns: false,
                    forward: false,
                },
            },
            host: Host {
                nested_virtualization: false,
                rosetta: false,
            },
        };
        let backend = machine_backend_from_vm_spec(&spec).expect("backend should resolve");
        assert_eq!(backend, Backend::CloudHypervisor);
    }
}
