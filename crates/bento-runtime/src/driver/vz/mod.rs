use std::os::fd::OwnedFd;
use std::time::Duration;

use objc2::AllocAnyThread;
use objc2::{rc::Retained, ClassType};
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_virtualization::{
    VZDirectorySharingDeviceConfiguration, VZDiskImageStorageDeviceAttachment,
    VZFileSerialPortAttachment, VZGenericPlatformConfiguration, VZLinuxBootLoader,
    VZSharedDirectory, VZSingleDirectoryShare, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioFileSystemDeviceConfiguration, VZVirtioSocketDeviceConfiguration,
    VZVirtioTraditionalMemoryBalloonDeviceConfiguration, VZVirtualMachineConfiguration,
};

use crate::driver::vz::vm::get_machine_identifier;
use crate::{
    driver::{vz::utils::vz_nested_virtualization_is_supported, Driver, DriverError},
    instance::{resolve_mount_location, Instance, InstanceFile},
};

mod dispatch;
mod utils;
mod vm;
mod vz;

use vm::VirtualMachine;
pub use vm::VirtualMachineError;

#[derive(Debug)]
pub struct VzDriver {
    instance: Instance,
    vm: Option<VirtualMachine>,
}

impl VzDriver {
    pub fn new(instance: Instance) -> Self {
        Self { instance, vm: None }
    }
}

impl Driver for VzDriver {
    fn validate(&self) -> Result<(), DriverError> {
        if !utils::is_os_version_at_least(11, 0, 0) {
            return Err(DriverError::Backend(
                "Virtualization.framework requires macOS 11+".into(),
            ));
        }

        if !utils::vz_virtual_machine_is_supported() {
            return Err(DriverError::Backend(
                "Virtualization.framework is not supported on this system.".into(),
            ));
        }

        Ok(())
    }

    fn create(&self) -> Result<(), DriverError> {
        let _ = get_machine_identifier(&self.instance)?;
        Ok(())
    }

    fn start(&mut self) -> Result<(), DriverError> {
        self.validate()?;

        unsafe {
            let config = create_vm_config(&self.instance)?;
            let vm = start_vm(config)?;
            self.vm = Some(vm);
        }

        Ok(())
    }

    fn stop(&mut self) -> Result<(), DriverError> {
        self.vm.as_ref().map(|vm| vm.stop());
        Ok(())
    }

    fn open_vsock_stream(&self, port: u32) -> Result<OwnedFd, DriverError> {
        let vm = self.vm.as_ref().ok_or_else(|| {
            DriverError::Backend("cannot open vsock stream because VM is not running".to_string())
        })?;

        vm.open_vsock_stream(port).map_err(DriverError::from)
    }
}

unsafe fn start_vm(
    config: Retained<VZVirtualMachineConfiguration>,
) -> Result<VirtualMachine, DriverError> {
    let vm = VirtualMachine::new(config);

    vm.start()?;

    let events = vm.subscribe_state();
    let startup_timeout = Duration::from_mins(5);

    loop {
        let e = match events.recv_timeout(startup_timeout) {
            Ok(event) => event,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                return Err(DriverError::Backend(format!(
                    "timed out after {:?} waiting for VM to enter running state (current state: {})",
                    startup_timeout,
                    vm.state()
                )));
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                return Err(DriverError::Backend(
                    "VM state subscription disconnected while waiting for startup".to_string(),
                ));
            }
        };

        match e {
            vm::VirtualMachineState::Stopped => {
                return Err(DriverError::Backend(
                    "VM stopped unexpectedly during startup".to_string(),
                ));
            }
            vm::VirtualMachineState::Running => return Ok(vm),
            // TODO: add some trace logging here
            _ => continue,
        }
    }
}

unsafe fn create_vm_config(
    inst: &Instance,
) -> Result<Retained<VZVirtualMachineConfiguration>, DriverError> {
    let config = &inst.config;
    let machine_id = get_machine_identifier(&inst)?;
    let boot_assets = inst.boot_assets()?;

    let bootloader = VZLinuxBootLoader::new();

    let kernel = NSString::from_str(&boot_assets.kernel.to_string_lossy());
    let kernel_url = NSURL::initFileURLWithPath(NSURL::alloc(), &kernel);
    bootloader.setKernelURL(&kernel_url);

    let initramfs = NSString::from_str(&boot_assets.initramfs.to_string_lossy());
    let initramfs_url = NSURL::initFileURLWithPath(NSURL::alloc(), &initramfs);
    bootloader.setInitialRamdiskURL(Some(&initramfs_url));

    // FIX: How should I handle this?
    let command_line = "console=hvc0 rd.break=initqueue root=/dev/vda";
    bootloader.setCommandLine(&NSString::from_str(&command_line));

    // TODO: rename this var, can't be config because otherwise I'll shadow
    let c = VZVirtualMachineConfiguration::new();
    c.setBootLoader(Some(&bootloader));

    let cpu_count = config.cpus.unwrap_or(2);
    if cpu_count <= 0 {
        return Err(DriverError::Backend(format!(
            "invalid CPU count in instance config: {cpu_count}"
        )));
    }
    c.setCPUCount(cpu_count as usize);

    let memory_mib = config.memory.unwrap_or(2048);
    if memory_mib <= 0 {
        return Err(DriverError::Backend(format!(
            "invalid memory size in MiB in instance config: {memory_mib}"
        )));
    }
    c.setMemorySize((memory_mib as u64) * 1024 * 1024);

    // NOTE: Attach platform configuration
    let platform_config = VZGenericPlatformConfiguration::new();
    platform_config.setMachineIdentifier(&machine_id);

    if config.nested_virtualization.unwrap_or(false) {
        if !utils::is_os_version_at_least(15, 0, 0) {
            return Err(DriverError::Backend(
                "nested virtualization requires macOS 15 or newer".to_string(),
            ));
        }

        if !vz_nested_virtualization_is_supported() {
            return Err(DriverError::Backend(
                "nested virtualization is not supported on this device".to_string(),
            ));
        }

        platform_config.setNestedVirtualizationEnabled(true);
    }

    c.setPlatform(&platform_config);

    // NOTE: Attach serial configuration
    let s = NSString::from_str(&inst.file(InstanceFile::SerialLog).to_string_lossy());
    let serial_url = NSURL::initFileURLWithPath(NSURL::alloc(), &s);
    let attachment = VZFileSerialPortAttachment::initWithURL_append_error(
        VZFileSerialPortAttachment::alloc(),
        &serial_url,
        false,
    )
    .map_err(|nse| DriverError::Backend(nse.to_string()))?;

    let console = VZVirtioConsoleDeviceSerialPortConfiguration::new();
    console.setAttachment(Some(&attachment));
    let serial_ports = NSArray::from_slice(&[console.as_super()]);
    c.setSerialPorts(&serial_ports);

    attach_devices(&c);
    attach_storage_devices(&c, inst)?;
    attach_directory_shares(&c, inst)?;

    c.validateWithError()
        .map_err(|nse| DriverError::Backend(nse.to_string()))?;

    return Ok(c);
}

unsafe fn attach_directory_shares(
    config: &Retained<VZVirtualMachineConfiguration>,
    inst: &Instance,
) -> Result<(), DriverError> {
    if inst.config.mounts.is_empty() {
        return Ok(());
    }

    let mut share_configs = Vec::with_capacity(inst.config.mounts.len());
    for (index, mount) in inst.config.mounts.iter().enumerate() {
        let host_location = resolve_mount_location(&mount.location)
            .map_err(|reason| DriverError::Backend(format!("invalid mount location: {reason}")))?;

        let metadata = std::fs::metadata(&host_location).map_err(|err| {
            DriverError::Backend(format!(
                "failed to access shared directory {}: {err}",
                host_location.display()
            ))
        })?;
        if !metadata.is_dir() {
            return Err(DriverError::Backend(format!(
                "shared directory path is not a directory: {}",
                host_location.display()
            )));
        }

        let host = NSString::from_str(&host_location.to_string_lossy());
        let host_url = NSURL::initFileURLWithPath(NSURL::alloc(), &host);

        let shared_directory = VZSharedDirectory::initWithURL_readOnly(
            VZSharedDirectory::alloc(),
            &host_url,
            !mount.writable,
        );
        let share = VZSingleDirectoryShare::initWithDirectory(
            VZSingleDirectoryShare::alloc(),
            &shared_directory,
        );

        let tag = NSString::from_str(&format!("mount{index}"));
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
    let devices = NSArray::from_slice(&[entropy.as_super()]);
    config.setEntropyDevices(&devices);

    let balloon = VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new();
    let devices = NSArray::from_slice(&[balloon.as_super()]);
    config.setMemoryBalloonDevices(&devices);

    let socket = VZVirtioSocketDeviceConfiguration::new();
    let devices = NSArray::from_slice(&[socket.as_super()]);
    config.setSocketDevices(&devices);
}

unsafe fn attach_storage_devices(
    config: &Retained<VZVirtualMachineConfiguration>,
    inst: &Instance,
) -> Result<(), DriverError> {
    let mut disks = Vec::new();
    if let Some(root_disk) = inst.root_disk()? {
        disks.push(root_disk);
    }
    disks.extend(inst.data_disks()?);

    if disks.is_empty() {
        return Ok(());
    }

    let mut storage_configs = Vec::with_capacity(disks.len());
    for disk in disks {
        let metadata = std::fs::metadata(&disk.path).map_err(|err| {
            DriverError::Backend(format!(
                "failed to access disk image {}: {err}",
                disk.path.display()
            ))
        })?;

        if !metadata.is_file() {
            return Err(DriverError::Backend(format!(
                "disk image path is not a file: {}",
                disk.path.display()
            )));
        }

        let disk_path = NSString::from_str(&disk.path.to_string_lossy());
        let disk_url = NSURL::initFileURLWithPath(NSURL::alloc(), &disk_path);

        let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &disk_url,
            disk.read_only,
        )
        .map_err(|nse| {
            DriverError::Backend(format!(
                "failed to initialize disk image attachment {}: {}",
                disk.path.display(),
                nse
            ))
        })?;

        let storage = VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &attachment,
        );
        storage_configs.push(storage);
    }

    let storage_refs: Vec<_> = storage_configs.iter().map(|cfg| cfg.as_super()).collect();
    let storage_devices = NSArray::from_slice(&storage_refs);
    config.setStorageDevices(&storage_devices);

    Ok(())
}
