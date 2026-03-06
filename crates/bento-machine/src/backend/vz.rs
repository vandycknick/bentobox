use std::os::fd::{IntoRawFd, OwnedFd};
use std::time::Duration;

use bento_protocol::{DEFAULT_DISCOVERY_PORT, KERNEL_PARAM_DISCOVERY_PORT};
use nix::unistd::pipe;
use objc2::AllocAnyThread;
use objc2::{rc::Retained, ClassType};
use objc2_foundation::{NSArray, NSFileHandle, NSString, NSURL};
use objc2_virtualization::{
    VZDirectorySharingDeviceConfiguration, VZDiskImageStorageDeviceAttachment,
    VZFileHandleSerialPortAttachment, VZGenericPlatformConfiguration, VZLinuxBootLoader,
    VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration, VZSharedDirectory,
    VZSingleDirectoryShare, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioFileSystemDeviceConfiguration, VZVirtioNetworkDeviceConfiguration,
    VZVirtioSocketDeviceConfiguration, VZVirtioTraditionalMemoryBalloonDeviceConfiguration,
    VZVirtualMachineConfiguration,
};

use crate::backend::MachineBackend;
use crate::types::{
    MachineConfig, MachineError, MachineId, MachineKind, MachineState, NetworkMode,
    OpenDeviceRequest, OpenDeviceResponse, ResolvedMachineSpec,
};

mod dispatch;
mod objc_ext;
mod utils;
mod vm;

use vm::{get_machine_identifier, VirtualMachine, VirtualMachineState};

struct SerialHostPipes {
    guest_input: OwnedFd,
    guest_output: OwnedFd,
}

struct VmBootstrap {
    config: Retained<VZVirtualMachineConfiguration>,
    serial: SerialHostPipes,
}

#[derive(Debug)]
pub(crate) struct VzMachineBackend {
    spec: ResolvedMachineSpec,
    vm: Option<VirtualMachine>,
    state: MachineState,
}

impl VzMachineBackend {
    pub(crate) fn new(spec: ResolvedMachineSpec) -> Result<Self, MachineError> {
        validate(&spec)?;
        Ok(Self {
            spec,
            vm: None,
            state: MachineState::Created,
        })
    }
}

impl MachineBackend for VzMachineBackend {
    fn state(&self) -> Result<MachineState, MachineError> {
        Ok(match self.vm.as_ref() {
            Some(vm) => vm.state().into(),
            None => self.state,
        })
    }

    fn start(&mut self) -> Result<(), MachineError> {
        validate_support()?;
        if self.vm.is_some() {
            return Ok(());
        }

        unsafe {
            let config = create_vm_config(&self.spec)?;
            let vm = start_vm(config)?;
            self.vm = Some(vm);
        }

        self.state = MachineState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MachineError> {
        if let Some(vm) = self.vm.as_ref() {
            unsafe {
                stop_vm(vm)?;
            }
        }

        self.vm = None;
        self.state = MachineState::Stopped;
        Ok(())
    }

    fn open_device(&self, request: OpenDeviceRequest) -> Result<OpenDeviceResponse, MachineError> {
        let vm = self.vm.as_ref().ok_or_else(|| {
            MachineError::Backend(format!(
                "cannot open device because machine {:?} is not running",
                self.spec.id.as_str()
            ))
        })?;

        match request {
            OpenDeviceRequest::Vsock { port } => vm
                .open_vsock_stream(port)
                .map(|stream| OpenDeviceResponse::Vsock { stream })
                .map_err(MachineError::from),
            OpenDeviceRequest::Serial => vm
                .open_serial_fds()
                .map(|(guest_input, guest_output)| OpenDeviceResponse::Serial {
                    guest_input,
                    guest_output,
                })
                .map_err(MachineError::from),
        }
    }
}

fn validate_support() -> Result<(), MachineError> {
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

unsafe fn start_vm(bootstrap: VmBootstrap) -> Result<VirtualMachine, MachineError> {
    let vm = VirtualMachine::new(
        bootstrap.config,
        bootstrap.serial.guest_input,
        bootstrap.serial.guest_output,
    );

    vm.start()?;

    let events = vm.subscribe_state();
    let startup_timeout = Duration::from_secs(60 * 5);
    loop {
        let event = match events.recv_timeout(startup_timeout) {
            Ok(event) => event,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                return Err(MachineError::Backend(format!(
                    "timed out after {:?} waiting for machine to enter running state (current state: {})",
                    startup_timeout,
                    vm.state()
                )));
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                return Err(MachineError::Backend(
                    "machine state subscription disconnected while waiting for startup".to_string(),
                ));
            }
        };

        match event {
            VirtualMachineState::Stopped => {
                return Err(MachineError::Backend(
                    "machine stopped unexpectedly during startup".to_string(),
                ));
            }
            VirtualMachineState::Running => return Ok(vm),
            _ => continue,
        }
    }
}

unsafe fn stop_vm(vm: &VirtualMachine) -> Result<(), MachineError> {
    if vm.state() == VirtualMachineState::Stopped {
        return Ok(());
    }

    vm.stop()?;
    let events = vm.subscribe_state();
    let shutdown_timeout = Duration::from_secs(60 * 5);

    loop {
        let event = match events.recv_timeout(shutdown_timeout) {
            Ok(event) => event,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                return Err(MachineError::Backend(format!(
                    "timed out after {:?} waiting for machine to stop (current state: {})",
                    shutdown_timeout,
                    vm.state()
                )));
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                return Err(MachineError::Backend(
                    "machine state subscription disconnected while waiting for shutdown"
                        .to_string(),
                ));
            }
        };

        match event {
            VirtualMachineState::Stopped => return Ok(()),
            VirtualMachineState::Error => {
                return Err(MachineError::Backend(
                    "machine entered error state while stopping".to_string(),
                ));
            }
            _ => continue,
        }
    }
}

unsafe fn create_vm_config(spec: &ResolvedMachineSpec) -> Result<VmBootstrap, MachineError> {
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

    let command_line = format!(
        "console=hvc0 rd.break=initqueue root=/dev/vda {}={}",
        KERNEL_PARAM_DISCOVERY_PORT, DEFAULT_DISCOVERY_PORT,
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
    if machine.mounts.is_empty() {
        return Ok(());
    }

    let mut share_configs = Vec::with_capacity(machine.mounts.len());
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
        let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &disk_url,
            disk.read_only,
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
    field: &str,
) -> Result<&'a std::path::Path, MachineError> {
    path.map(|value| value.as_path())
        .ok_or_else(|| MachineError::InvalidConfig {
            id: id.clone(),
            reason: format!("{field} must be configured for a VZ machine"),
        })
}

fn machine_identifier_path<'a>(
    _id: &str,
    config: &'a MachineConfig,
) -> Result<&'a std::path::Path, MachineError> {
    config.machine_identifier_path.as_deref().ok_or_else(|| {
        MachineError::Backend(
            "machine_identifier_path must be configured for a VZ machine".to_string(),
        )
    })
}

fn validate_machine_config(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    let config = &spec.config;

    let kernel_path = required_path(&spec.id, config.kernel_path.as_ref(), "kernel_path")?;
    let initramfs_path = required_path(&spec.id, config.initramfs_path.as_ref(), "initramfs_path")?;
    let machine_identifier_path = machine_identifier_path(spec.id.as_str(), config)?;

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

    if let Some(parent) = machine_identifier_path.parent() {
        if !parent.is_dir() {
            return Err(MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: format!(
                    "machine identifier parent directory does not exist: {}",
                    parent.display()
                ),
            });
        }
    }

    let cpu_count = config.cpus.unwrap_or(2);
    if cpu_count == 0 {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "cpu count must be greater than zero".to_string(),
        });
    }

    let memory_mib = config.memory_mib.unwrap_or(2048);
    if memory_mib == 0 {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "memory_mib must be greater than zero".to_string(),
        });
    }

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
    }

    match config.network {
        NetworkMode::VzNat | NetworkMode::None => {}
        NetworkMode::Bridged => {
            return Err(MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: "network mode 'bridged' is not implemented yet".to_string(),
            });
        }
        NetworkMode::Cni => {
            return Err(MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: "network mode 'cni' is not implemented yet".to_string(),
            });
        }
    }

    for disk in config.root_disk.iter().chain(config.data_disks.iter()) {
        let metadata =
            std::fs::metadata(&disk.path).map_err(|err| MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: format!("failed to access disk image {}: {err}", disk.path.display()),
            })?;

        if !metadata.is_file() {
            return Err(MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: format!("disk image path is not a file: {}", disk.path.display()),
            });
        }
    }

    for mount in &config.mounts {
        let metadata =
            std::fs::metadata(&mount.host_path).map_err(|err| MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: format!(
                    "failed to access shared directory {}: {err}",
                    mount.host_path.display()
                ),
            })?;

        if !metadata.is_dir() {
            return Err(MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: format!(
                    "shared directory path is not a directory: {}",
                    mount.host_path.display()
                ),
            });
        }
    }

    Ok(())
}
