use bento_vmm::{
    Backend, DiskImage, MachineIdentifier, NetworkMode, SharedDirectory, VmConfig, VmmError,
};
use thiserror::Error;

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
}

#[derive(Debug, Clone)]
pub(crate) struct InstanceVmConfig {
    pub config: VmConfig,
    pub machine_identifier: Option<MachineIdentifier>,
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

pub(crate) fn machine_backend(engine: EngineType) -> Result<Backend, VmmError> {
    match engine {
        EngineType::VZ => Ok(Backend::Vz),
        EngineType::Firecracker => Ok(Backend::Firecracker),
        EngineType::CloudHypervisor => Ok(Backend::CloudHypervisor),
    }
}

pub(crate) fn machine_identifier_path(inst: &Instance) -> std::path::PathBuf {
    inst.file(InstanceFile::AppleMachineIdentifier)
}

fn load_machine_identifier(inst: &Instance) -> Result<MachineIdentifier, MachineSpecError> {
    let path = machine_identifier_path(inst);
    match std::fs::read(path) {
        Ok(bytes) => Ok(MachineIdentifier::from_bytes(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(MachineIdentifier::new()),
        Err(err) => Err(err.into()),
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
    use super::machine_backend;
    use bento_runtime::instance::EngineType;
    use bento_vmm::Backend;

    #[test]
    fn machine_backend_maps_cloud_hypervisor_engine() {
        let backend = machine_backend(EngineType::CloudHypervisor).expect("backend should resolve");
        assert_eq!(backend, Backend::CloudHypervisor);
    }
}
