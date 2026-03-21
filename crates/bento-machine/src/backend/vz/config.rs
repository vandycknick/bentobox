use std::os::fd::{IntoRawFd, OwnedFd};

use bento_protocol::{DEFAULT_DISCOVERY_PORT, KERNEL_PARAM_DISCOVERY_PORT};
use nix::unistd::pipe;
use objc2::AllocAnyThread;
use objc2::{rc::Retained, ClassType};
use objc2_foundation::{NSArray, NSFileHandle, NSString, NSURL};
use objc2_virtualization::{
    VZDirectorySharingDeviceConfiguration, VZDiskImageCachingMode,
    VZDiskImageStorageDeviceAttachment, VZDiskImageSynchronizationMode,
    VZFileHandleSerialPortAttachment, VZGenericPlatformConfiguration, VZLinuxBootLoader,
    VZLinuxRosettaDirectoryShare, VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration,
    VZSharedDirectory, VZSingleDirectoryShare, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioFileSystemDeviceConfiguration, VZVirtioNetworkDeviceConfiguration,
    VZVirtioSocketDeviceConfiguration, VZVirtioTraditionalMemoryBalloonDeviceConfiguration,
    VZVirtualMachineConfiguration,
};

use crate::backend::vz::utils;
use crate::backend::vz::vm::get_machine_identifier;
use crate::types::{
    MachineConfig, MachineError, MachineId, MachineKind, NetworkMode, ResolvedMachineSpec,
};

const BENTO_ROSETTA_TAG: &str = "bento-rosetta";
const ROSETTA_INSTALL_HINT: &str =
    "Rosetta for Linux VMs is not installed on this host. Install it with: softwareupdate --install-rosetta";

pub(super) struct SerialHostPipes {
    pub(super) guest_input: OwnedFd,
    pub(super) guest_output: OwnedFd,
}

pub(super) struct VmBootstrap {
    pub(super) config: Retained<VZVirtualMachineConfiguration>,
    pub(super) serial: SerialHostPipes,
}

pub(crate) fn validate(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    validate_support()?;
    validate_machine_config(spec)
}

pub(crate) fn prepare(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    validate(spec)?;
    let path = machine_identifier_path(spec.id.as_str(), &spec.config)?;
    let _ = get_machine_identifier(path)?;
    Ok(())
}

pub(super) fn validate_support() -> Result<(), MachineError> {
    if !utils::is_os_version_at_least(11, 0, 0) {
        return Err(MachineError::UnsupportedBackend {
            kind: MachineKind::Vz,
            reason: "Virtualization.framework requires macOS 11+".into(),
        });
    }

    if !utils::vz_virtual_machine_is_supported() {
        return Err(MachineError::UnsupportedBackend {
            kind: MachineKind::Vz,
            reason: "Virtualization.framework is not supported on this system".into(),
        });
    }

    Ok(())
}

pub(super) unsafe fn create_vm_config(
    spec: &ResolvedMachineSpec,
) -> Result<VmBootstrap, MachineError> {
    let config = &spec.config;
    let machine_id = get_machine_identifier(machine_identifier_path(spec.id.as_str(), config)?)?;
    let kernel_path = required_path(&spec.id, config.kernel_path.as_ref(), "kernel_path")?;
    let initramfs_path = required_path(&spec.id, config.initramfs_path.as_ref(), "initramfs_path")?;

    if !kernel_path.is_file() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: format!("kernel path is not a file: {}", kernel_path.display()),
        });
    }
    if !initramfs_path.is_file() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: format!("initramfs path is not a file: {}", initramfs_path.display()),
        });
    }

    let bootloader = VZLinuxBootLoader::new();
    let kernel = NSString::from_str(&kernel_path.to_string_lossy());
    let kernel_url = NSURL::initFileURLWithPath(NSURL::alloc(), &kernel);
    bootloader.setKernelURL(&kernel_url);

    let initramfs = NSString::from_str(&initramfs_path.to_string_lossy());
    let initramfs_url = NSURL::initFileURLWithPath(NSURL::alloc(), &initramfs);
    bootloader.setInitialRamdiskURL(Some(&initramfs_url));

    let root_arg = config
        .root_disk
        .as_ref()
        .map(|_| "root=/dev/vda")
        .unwrap_or("");
    let command_line = format!(
        "console=hvc0 rd.break=initqueue {} {}={}",
        root_arg, KERNEL_PARAM_DISCOVERY_PORT, DEFAULT_DISCOVERY_PORT,
    );
    bootloader.setCommandLine(&NSString::from_str(&command_line));

    let machine_config = VZVirtualMachineConfiguration::new();
    machine_config.setBootLoader(Some(&bootloader));

    let cpu_count = config.cpus.unwrap_or(2);
    if cpu_count == 0 {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "cpu count must be greater than zero".to_string(),
        });
    }
    machine_config.setCPUCount(cpu_count);

    let memory_mib = config.memory_mib.unwrap_or(2048);
    if memory_mib == 0 {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "memory_mib must be greater than zero".to_string(),
        });
    }
    machine_config.setMemorySize(memory_mib * 1024 * 1024);

    let platform_config = VZGenericPlatformConfiguration::new();
    platform_config.setMachineIdentifier(&machine_id);

    if config.nested_virtualization {
        if !utils::is_os_version_at_least(15, 0, 0) {
            return Err(MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: "nested virtualization requires macOS 15 or newer".to_string(),
            });
        }

        if !utils::vz_nested_virtualization_is_supported() {
            return Err(MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: "nested virtualization is not supported on this device".to_string(),
            });
        }

        platform_config.setNestedVirtualizationEnabled(true);
    }

    validate_rosetta_config(&spec.id, config)?;

    machine_config.setPlatform(&platform_config);

    let (guest_serial_read, host_serial_write) =
        pipe().map_err(|err| MachineError::Backend(format!("create serial input pipe: {err}")))?;
    let (host_serial_read, guest_serial_write) =
        pipe().map_err(|err| MachineError::Backend(format!("create serial output pipe: {err}")))?;

    let serial_read_handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
        NSFileHandle::alloc(),
        guest_serial_read.into_raw_fd(),
        true,
    );
    let serial_write_handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
        NSFileHandle::alloc(),
        guest_serial_write.into_raw_fd(),
        true,
    );

    let attachment =
        VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
            VZFileHandleSerialPortAttachment::alloc(),
            Some(&serial_read_handle),
            Some(&serial_write_handle),
        );

    let console = VZVirtioConsoleDeviceSerialPortConfiguration::new();
    console.setAttachment(Some(&attachment));
    let serial_ports = NSArray::from_slice(&[console.as_super()]);
    machine_config.setSerialPorts(&serial_ports);

    attach_devices(&machine_config);
    attach_network_devices(&machine_config, &spec.id, config)?;
    attach_storage_devices(&machine_config, &spec.id, config)?;
    attach_directory_shares(&machine_config, &spec.id, config)?;

    machine_config
        .validateWithError()
        .map_err(|err| MachineError::Backend(err.to_string()))?;

    Ok(VmBootstrap {
        config: machine_config,
        serial: SerialHostPipes {
            guest_input: host_serial_write,
            guest_output: host_serial_read,
        },
    })
}

unsafe fn attach_network_devices(
    config: &Retained<VZVirtualMachineConfiguration>,
    id: &MachineId,
    machine: &MachineConfig,
) -> Result<(), MachineError> {
    match machine.network {
        NetworkMode::VzNat => {
            let network = VZVirtioNetworkDeviceConfiguration::new();
            let attachment = VZNATNetworkDeviceAttachment::new();
            network.setAttachment(Some(attachment.as_super()));

            let refs: [&VZNetworkDeviceConfiguration; 1] = [network.as_super()];
            let devices = NSArray::from_slice(&refs);
            config.setNetworkDevices(&devices);
            Ok(())
        }
        NetworkMode::None => Ok(()),
        NetworkMode::Bridged => Err(MachineError::InvalidConfig {
            id: id.clone(),
            reason: "network mode 'bridged' is not implemented yet".into(),
        }),
        NetworkMode::Cni => Err(MachineError::InvalidConfig {
            id: id.clone(),
            reason: "network mode 'cni' is not implemented yet".into(),
        }),
    }
}

unsafe fn attach_directory_shares(
    config: &Retained<VZVirtualMachineConfiguration>,
    id: &MachineId,
    machine: &MachineConfig,
) -> Result<(), MachineError> {
    if machine.mounts.is_empty() && !machine.rosetta {
        return Ok(());
    }

    let mut share_configs = Vec::with_capacity(machine.mounts.len() + usize::from(machine.rosetta));
    for mount in &machine.mounts {
        let metadata =
            std::fs::metadata(&mount.host_path).map_err(|err| MachineError::InvalidConfig {
                id: id.clone(),
                reason: format!(
                    "failed to access shared directory {}: {err}",
                    mount.host_path.display()
                ),
            })?;
        if !metadata.is_dir() {
            return Err(MachineError::InvalidConfig {
                id: id.clone(),
                reason: format!(
                    "shared directory path is not a directory: {}",
                    mount.host_path.display()
                ),
            });
        }

        let host = NSString::from_str(&mount.host_path.to_string_lossy());
        let host_url = NSURL::initFileURLWithPath(NSURL::alloc(), &host);
        let shared_directory = VZSharedDirectory::initWithURL_readOnly(
            VZSharedDirectory::alloc(),
            &host_url,
            mount.read_only,
        );
        let share = VZSingleDirectoryShare::initWithDirectory(
            VZSingleDirectoryShare::alloc(),
            &shared_directory,
        );

        let tag = NSString::from_str(&mount.tag);
        let fs = VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        );
        fs.setShare(Some(&share));
        share_configs.push(fs);
    }

    if machine.rosetta {
        let tag = NSString::from_str(BENTO_ROSETTA_TAG);
        let fs = VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        );
        let share =
            VZLinuxRosettaDirectoryShare::initWithError(VZLinuxRosettaDirectoryShare::alloc())
                .map_err(|err| {
                    MachineError::Backend(format!(
                        "failed to initialize Rosetta directory share: {err}"
                    ))
                })?;
        fs.setShare(Some(&share));
        share_configs.push(fs);
    }

    let refs: Vec<&VZDirectorySharingDeviceConfiguration> =
        share_configs.iter().map(|cfg| cfg.as_super()).collect();
    let devices = NSArray::from_slice(&refs);
    config.setDirectorySharingDevices(&devices);
    Ok(())
}

unsafe fn attach_devices(config: &Retained<VZVirtualMachineConfiguration>) {
    let entropy = VZVirtioEntropyDeviceConfiguration::new();
    config.setEntropyDevices(&NSArray::from_slice(&[entropy.as_super()]));

    let balloon = VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new();
    config.setMemoryBalloonDevices(&NSArray::from_slice(&[balloon.as_super()]));

    let socket = VZVirtioSocketDeviceConfiguration::new();
    config.setSocketDevices(&NSArray::from_slice(&[socket.as_super()]));
}

unsafe fn attach_storage_devices(
    config: &Retained<VZVirtualMachineConfiguration>,
    id: &MachineId,
    machine: &MachineConfig,
) -> Result<(), MachineError> {
    let mut disks = Vec::new();
    if let Some(root_disk) = machine.root_disk.as_ref() {
        disks.push(root_disk);
    }
    for disk in &machine.data_disks {
        disks.push(disk);
    }

    if disks.is_empty() {
        return Ok(());
    }

    let mut storage_configs = Vec::with_capacity(disks.len());
    for disk in disks {
        let metadata =
            std::fs::metadata(&disk.path).map_err(|err| MachineError::InvalidConfig {
                id: id.clone(),
                reason: format!("failed to access disk image {}: {err}", disk.path.display()),
            })?;

        if !metadata.is_file() {
            return Err(MachineError::InvalidConfig {
                id: id.clone(),
                reason: format!("disk image path is not a file: {}", disk.path.display()),
            });
        }

        let disk_path = NSString::from_str(&disk.path.to_string_lossy());
        let disk_url = NSURL::initFileURLWithPath(NSURL::alloc(), &disk_path);
        let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_cachingMode_synchronizationMode_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &disk_url,
            disk.read_only,
            VZDiskImageCachingMode::Cached,
            VZDiskImageSynchronizationMode::Full,
        )
        .map_err(|err| {
            MachineError::Backend(format!(
                "failed to initialize disk image attachment {}: {}",
                disk.path.display(),
                err
            ))
        })?;

        let storage = VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        );
        storage_configs.push(storage);
    }

    let refs: Vec<_> = storage_configs.iter().map(|cfg| cfg.as_super()).collect();
    config.setStorageDevices(&NSArray::from_slice(&refs));
    Ok(())
}

fn required_path<'a>(
    id: &MachineId,
    path: Option<&'a std::path::PathBuf>,
    field: &'static str,
) -> Result<&'a std::path::Path, MachineError> {
    path.map(|path| path.as_path())
        .ok_or_else(|| MachineError::InvalidConfig {
            id: id.clone(),
            reason: format!("{field} must be set"),
        })
}

fn validate_machine_config(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    if spec.config.machine_directory.as_os_str().is_empty() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "machine_directory must be set".to_string(),
        });
    }

    let _ = required_path(&spec.id, spec.config.kernel_path.as_ref(), "kernel_path")?;
    let _ = required_path(
        &spec.id,
        spec.config.initramfs_path.as_ref(),
        "initramfs_path",
    )?;
    Ok(())
}

fn machine_identifier_path<'a>(
    id: &str,
    config: &'a MachineConfig,
) -> Result<&'a std::path::Path, MachineError> {
    config
        .machine_identifier_path
        .as_deref()
        .ok_or_else(|| MachineError::InvalidConfig {
            id: MachineId::from(id),
            reason: "machine_identifier_path must be set for VZ".to_string(),
        })
}

fn validate_rosetta_config(id: &MachineId, config: &MachineConfig) -> Result<(), MachineError> {
    if !config.rosetta {
        return Ok(());
    }

    if !utils::is_os_version_at_least(13, 0, 0) {
        return Err(MachineError::InvalidConfig {
            id: id.clone(),
            reason: "rosetta requires macOS 13 or newer".to_string(),
        });
    }

    match utils::vz_rosetta_availability() {
        utils::RosettaAvailability::Installed => {}
        utils::RosettaAvailability::NotInstalled => {
            return Err(MachineError::InvalidConfig {
                id: id.clone(),
                reason: ROSETTA_INSTALL_HINT.to_string(),
            });
        }
        utils::RosettaAvailability::NotSupported => {
            return Err(MachineError::InvalidConfig {
                id: id.clone(),
                reason: "rosetta is not supported on this host".to_string(),
            });
        }
    }

    Ok(())
}
