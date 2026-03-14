use bento_machine::{
    DiskImage, Machine, MachineConfig, MachineError, MachineId, MachineKind, MachineSpec,
    NetworkMode, SharedDirectory,
};
use thiserror::Error;

use bento_runtime::instance::{
    resolve_mount_location, EngineType, Instance, InstanceFile, NetworkMode as InstanceNetworkMode,
};

#[derive(Debug, Error)]
pub enum MachineSpecError {
    #[error(transparent)]
    Machine(#[from] MachineError),

    #[error(transparent)]
    InstanceDisk(#[from] bento_runtime::instance::InstanceDiskError),

    #[error(transparent)]
    InstanceBoot(#[from] bento_runtime::instance::InstanceBootError),

    #[error("invalid mount location: {0}")]
    InvalidMountLocation(String),
}

pub fn prepare_instance(inst: &Instance) -> Result<(), MachineSpecError> {
    let spec = machine_spec_for_instance(inst)?;
    Machine::validate(&spec)?;
    Machine::prepare(&spec)?;
    Ok(())
}

pub(crate) fn machine_spec_for_instance(inst: &Instance) -> Result<MachineSpec, MachineSpecError> {
    Ok(MachineSpec {
        id: MachineId::from(inst.name.as_str()),
        kind: Some(machine_kind(inst.engine())?),
        config: machine_config_for_instance(inst)?,
    })
}

fn machine_kind(engine: EngineType) -> Result<MachineKind, MachineError> {
    match engine {
        EngineType::VZ => Ok(MachineKind::Vz),
    }
}

fn machine_config_for_instance(inst: &Instance) -> Result<MachineConfig, MachineSpecError> {
    let boot_assets = inst.boot_assets()?;
    let root_disk = inst.root_disk()?.map(|disk| DiskImage {
        path: disk.path,
        read_only: disk.read_only,
    });
    let data_disks = inst
        .data_disks()?
        .into_iter()
        .map(|disk| DiskImage {
            path: disk.path,
            read_only: disk.read_only,
        })
        .collect();
    let mounts = inst
        .config
        .mounts
        .iter()
        .enumerate()
        .map(|(index, mount)| {
            resolve_mount_location(&mount.location)
                .map_err(MachineSpecError::InvalidMountLocation)
                .map(|host_path| SharedDirectory {
                    host_path,
                    tag: format!("mount{index}"),
                    read_only: !mount.writable,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(MachineConfig {
        cpus: inst.config.cpus.map(|cpus| cpus as usize),
        memory_mib: inst.config.memory.map(|memory| memory as u64),
        kernel_path: Some(boot_assets.kernel),
        initramfs_path: Some(boot_assets.initramfs),
        machine_identifier_path: Some(inst.file(InstanceFile::AppleMachineIdentifier)),
        nested_virtualization: inst.config.nested_virtualization.unwrap_or(false),
        rosetta: inst.config.rosetta.unwrap_or(false),
        network: map_network_mode(inst.resolved_network_mode()),
        root_disk,
        data_disks,
        mounts,
    })
}

fn map_network_mode(mode: InstanceNetworkMode) -> NetworkMode {
    match mode {
        InstanceNetworkMode::VzNat => NetworkMode::VzNat,
        InstanceNetworkMode::None => NetworkMode::None,
        InstanceNetworkMode::Bridged => NetworkMode::Bridged,
        InstanceNetworkMode::Cni => NetworkMode::Cni,
    }
}
