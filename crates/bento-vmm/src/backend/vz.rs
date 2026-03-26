use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bento_protocol::{DEFAULT_DISCOVERY_PORT, KERNEL_PARAM_DISCOVERY_PORT};
use bento_vz::device::{
    EntropyDeviceConfiguration, LinuxRosettaDirectoryShare, MemoryBalloonDeviceConfiguration,
    NetworkDeviceConfiguration, SerialPortConfiguration, SharedDirectory, SingleDirectoryShare,
    SocketDevice, SocketDeviceConfiguration, StorageDeviceConfiguration,
    VirtioFileSystemDeviceConfiguration,
};
use bento_vz::{
    GenericMachineIdentifier, GenericPlatform, LinuxBootLoader, RosettaAvailability,
    VirtualMachine, VirtualMachineDelegate, VirtualMachineState, VzError,
};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use crate::stream::{MachineSerialStream, VsockStream};
use crate::types::{MachineIdentifier, NetworkMode, VmConfig, VmExit, VmmError};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const BENTO_ROSETTA_TAG: &str = "bento-rosetta";

#[derive(Debug)]
pub(crate) struct VzMachineBackend {
    config: VmConfig,
    inner: AsyncMutex<VzMachineState>,
    exit: Arc<Mutex<Option<VmExit>>>,
    exit_notify: Arc<Notify>,
}

#[derive(Debug)]
struct VzMachineState {
    vm: Option<VirtualMachine>,
    serial_port: Option<SerialPortConfiguration>,
}

impl VzMachineBackend {
    pub(crate) fn new(config: VmConfig) -> Result<Self, VmmError> {
        validate(&config)?;
        Ok(Self {
            config,
            inner: AsyncMutex::new(VzMachineState {
                vm: None,
                serial_port: None,
            }),
            exit: Arc::new(Mutex::new(None)),
            exit_notify: Arc::new(Notify::new()),
        })
    }

    pub(crate) async fn start(&self) -> Result<(), VmmError> {
        validate_support()?;
        let mut state = self.inner.lock().await;
        if state.vm.is_some() {
            return Err(VmmError::AlreadyRunning {
                name: self.config.name.clone(),
            });
        }

        let (vm, serial_port) = build_vm(&self.config)?;
        vm.set_delegate(ExitDelegate {
            exit: self.exit.clone(),
            notify: self.exit_notify.clone(),
        })
        .map_err(vz_error)?;
        let mut state_events = vm.subscribe_state();

        vm.start().await.map_err(vz_error)?;
        wait_for_state(
            &mut state_events,
            &vm,
            VirtualMachineState::Running,
            STARTUP_TIMEOUT,
        )
        .await?;

        state.vm = Some(vm);
        state.serial_port = Some(serial_port);
        Ok(())
    }

    pub(crate) async fn stop(&self) -> Result<(), VmmError> {
        let mut state = self.inner.lock().await;
        if let Some(vm) = state.vm.as_ref() {
            if vm.state() != VirtualMachineState::Stopped {
                let mut state_events = vm.subscribe_state();
                tracing::debug!(
                    machine_id = self.config.name.as_str(),
                    current_state = %vm.state(),
                    "starting VZ shutdown flow"
                );
                let graceful_stop_completed = if vm.can_request_stop() {
                    tracing::debug!(
                        machine_id = self.config.name.as_str(),
                        timeout = ?SHUTDOWN_TIMEOUT,
                        "requesting graceful VZ shutdown"
                    );
                    vm.request_stop().map_err(vz_error)?;
                    let graceful_result = wait_for_state(
                        &mut state_events,
                        vm,
                        VirtualMachineState::Stopped,
                        SHUTDOWN_TIMEOUT,
                    )
                    .await;
                    match &graceful_result {
                        Ok(()) => {
                            tracing::debug!(
                                machine_id = self.config.name.as_str(),
                                "graceful VZ shutdown completed"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                machine_id = self.config.name.as_str(),
                                error = %err,
                                timeout = ?SHUTDOWN_TIMEOUT,
                                "graceful VZ shutdown did not complete before timeout, falling back to hard stop"
                            );
                        }
                    }
                    graceful_result.is_ok()
                } else {
                    tracing::debug!(
                        machine_id = self.config.name.as_str(),
                        "guest does not support graceful request_stop, using hard stop"
                    );
                    false
                };

                if !graceful_stop_completed {
                    tracing::warn!(
                        machine_id = self.config.name.as_str(),
                        timeout = ?SHUTDOWN_TIMEOUT,
                        "executing hard VZ stop"
                    );
                    vm.stop().await.map_err(vz_error)?;
                    wait_for_state(
                        &mut state_events,
                        vm,
                        VirtualMachineState::Stopped,
                        SHUTDOWN_TIMEOUT,
                    )
                    .await?;
                    tracing::debug!(
                        machine_id = self.config.name.as_str(),
                        "hard VZ stop completed"
                    );
                }
            }
        }

        state.vm = None;
        state.serial_port = None;
        self.cache_exit(VmExit::Stopped)?;
        Ok(())
    }

    pub(crate) async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmmError> {
        let vm = {
            let state = self.inner.lock().await;
            state.vm.clone().ok_or_else(|| {
                VmmError::Backend(format!(
                    "cannot open vsock stream because machine {:?} is not running",
                    self.config.name.as_str()
                ))
            })?
        };

        let device = vm.open_devices().into_iter().next().ok_or_else(|| {
            VmmError::Backend("no virtio socket device configured in VM".to_string())
        })?;

        let stream = device.connect(port).await.map_err(vz_error)?;
        Ok(VsockStream::from_vz(stream))
    }

    pub(crate) async fn open_serial(&self) -> Result<MachineSerialStream, VmmError> {
        let serial_port = {
            let state = self.inner.lock().await;
            state.serial_port.clone().ok_or_else(|| {
                VmmError::Backend(format!(
                    "cannot open serial stream because machine {:?} is not running",
                    self.config.name.as_str()
                ))
            })?
        };

        let stream = serial_port.open_stream().map_err(vz_error)?;
        Ok(MachineSerialStream::from_vz(stream))
    }

    pub(crate) async fn wait(&self) -> Result<VmExit, VmmError> {
        loop {
            if let Some(exit) = self.cached_exit()? {
                return Ok(exit);
            }

            let maybe_vm = {
                let state = self.inner.lock().await;
                state.vm.clone()
            };

            let Some(vm) = maybe_vm else {
                return Err(VmmError::Backend(
                    "cannot wait for a virtual machine that has not been started".to_string(),
                ));
            };

            self.try_cache_exit_from_vm(&vm)?;
            if let Some(exit) = self.cached_exit()? {
                return Ok(exit);
            }

            self.exit_notify.notified().await;
        }
    }

    pub(crate) async fn try_wait(&self) -> Result<Option<VmExit>, VmmError> {
        if let Some(exit) = self.cached_exit()? {
            return Ok(Some(exit));
        }

        let maybe_vm = {
            let state = self.inner.lock().await;
            state.vm.clone()
        };

        let Some(vm) = maybe_vm else {
            return Ok(None);
        };

        self.try_cache_exit_from_vm(&vm)?;
        self.cached_exit()
    }

    fn cached_exit(&self) -> Result<Option<VmExit>, VmmError> {
        self.exit
            .lock()
            .map(|exit| exit.clone())
            .map_err(|_| VmmError::RegistryPoisoned)
    }

    fn cache_exit(&self, exit: VmExit) -> Result<(), VmmError> {
        let mut slot = self.exit.lock().map_err(|_| VmmError::RegistryPoisoned)?;
        if slot.is_none() {
            *slot = Some(exit);
            self.exit_notify.notify_waiters();
        }
        Ok(())
    }

    fn try_cache_exit_from_vm(&self, vm: &VirtualMachine) -> Result<(), VmmError> {
        match vm.state() {
            VirtualMachineState::Stopped => self.cache_exit(VmExit::Stopped),
            VirtualMachineState::Error => self.cache_exit(VmExit::StoppedWithError(
                "virtual machine entered error state".to_string(),
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone)]
struct ExitDelegate {
    exit: Arc<Mutex<Option<VmExit>>>,
    notify: Arc<Notify>,
}

impl VirtualMachineDelegate for ExitDelegate {
    fn guest_did_stop(&self) {
        if let Ok(mut slot) = self.exit.lock() {
            if slot.is_none() {
                *slot = Some(VmExit::Stopped);
            }
        }
        self.notify.notify_waiters();
    }

    fn did_stop_with_error(&self, error: VzError) {
        if let Ok(mut slot) = self.exit.lock() {
            if slot.is_none() {
                *slot = Some(VmExit::StoppedWithError(error.to_string()));
            }
        }
        self.notify.notify_waiters();
    }
}

pub(crate) fn validate(spec: &VmConfig) -> Result<(), VmmError> {
    validate_support()?;
    validate_machine_config(spec)
}

fn validate_support() -> Result<(), VmmError> {
    let _ = VirtualMachine::builder().map_err(vz_error)?;
    Ok(())
}

fn build_vm(spec: &VmConfig) -> Result<(VirtualMachine, SerialPortConfiguration), VmmError> {
    let serial_port = SerialPortConfiguration::virtio_console();

    let mut builder = VirtualMachine::builder()
        .map_err(vz_error)?
        .set_cpu_count(spec.cpus.unwrap_or(2))
        .set_memory_size(spec.memory_mib.unwrap_or(2048) * 1024 * 1024)
        .set_platform(build_platform(spec)?)
        .set_boot_loader(build_boot_loader(spec)?)
        .add_entropy_device(EntropyDeviceConfiguration::new())
        .add_memory_balloon_device(MemoryBalloonDeviceConfiguration::new())
        .add_serial_port(serial_port.clone())
        .add_socket_device(SocketDeviceConfiguration::new());

    if spec.network == NetworkMode::VzNat {
        builder = builder.add_network_device(NetworkDeviceConfiguration::nat());
    }

    if let Some(root_disk) = spec.root_disk.as_ref() {
        builder = builder.add_storage_device(
            StorageDeviceConfiguration::new(root_disk.path.clone(), root_disk.read_only)
                .map_err(vz_error)?,
        );
    }

    for disk in &spec.data_disks {
        builder = builder.add_storage_device(
            StorageDeviceConfiguration::new(disk.path.clone(), disk.read_only).map_err(vz_error)?,
        );
    }

    for mount in &spec.mounts {
        let shared_dir = SharedDirectory::new(mount.host_path.clone(), mount.read_only);
        let single_share = SingleDirectoryShare::new(shared_dir);
        let mut fs_config = VirtioFileSystemDeviceConfiguration::new(mount.tag.clone());
        fs_config.set_share(single_share);
        builder = builder.add_directory_share(fs_config);
    }

    if spec.rosetta {
        let mut rosetta_config = VirtioFileSystemDeviceConfiguration::new(BENTO_ROSETTA_TAG);
        rosetta_config.set_rosetta_share(LinuxRosettaDirectoryShare::new().map_err(vz_error)?);
        builder = builder.add_directory_share(rosetta_config);
    }

    let vm = builder.build().map_err(vz_error)?;
    Ok((vm, serial_port))
}

fn build_platform(spec: &VmConfig) -> Result<GenericPlatform, VmmError> {
    let mut platform = GenericPlatform::new();
    let machine_identifier = resolve_machine_identifier(spec)?;
    platform.set_machine_identifier(machine_identifier);
    platform.set_nested_virtualization_enabled(spec.nested_virtualization);
    Ok(platform)
}

fn build_boot_loader(spec: &VmConfig) -> Result<LinuxBootLoader, VmmError> {
    let kernel_path = required_path(&spec.name, spec.kernel_path.as_ref(), "kernel_path")?;
    let initramfs_path = required_path(&spec.name, spec.initramfs_path.as_ref(), "initramfs_path")?;

    let mut boot_loader = LinuxBootLoader::new(kernel_path);
    boot_loader.set_initial_ramdisk(initramfs_path);

    let root_arg = spec
        .root_disk
        .as_ref()
        .map(|_| " root=/dev/vda")
        .unwrap_or("");
    let command_line = format!(
        "console=hvc0 rd.break=initqueue{} {}={}",
        root_arg, KERNEL_PARAM_DISCOVERY_PORT, DEFAULT_DISCOVERY_PORT,
    );
    boot_loader.set_command_line(&command_line);
    Ok(boot_loader)
}

fn resolve_machine_identifier(config: &VmConfig) -> Result<GenericMachineIdentifier, VmmError> {
    let Some(machine_identifier) = config.machine_identifier.as_ref() else {
        return Ok(GenericMachineIdentifier::new());
    };

    if machine_identifier.is_empty() {
        let generated = GenericMachineIdentifier::new();
        machine_identifier.set_generated_bytes(generated.data())?;
        return Ok(generated);
    }

    GenericMachineIdentifier::from_bytes(&machine_identifier.bytes()).map_err(vz_error)
}

fn validate_machine_config(spec: &VmConfig) -> Result<(), VmmError> {
    if spec.base_directory.as_os_str().is_empty() {
        return Err(VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "base_directory must be set".to_string(),
        });
    }

    if spec.cpus == Some(0) {
        return Err(VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "cpu count must be greater than zero".to_string(),
        });
    }

    if spec.memory_mib == Some(0) {
        return Err(VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "memory_mib must be greater than zero".to_string(),
        });
    }

    let _ = required_path(&spec.name, spec.kernel_path.as_ref(), "kernel_path")?;
    let _ = required_path(&spec.name, spec.initramfs_path.as_ref(), "initramfs_path")?;

    if let Some(machine_identifier) = spec.machine_identifier.as_ref() {
        validate_machine_identifier(&spec.name, machine_identifier)?;
    }

    validate_nested_virtualization(spec)?;
    validate_rosetta(spec)?;

    match spec.network {
        NetworkMode::VzNat | NetworkMode::None => {}
        NetworkMode::Bridged => {
            return Err(VmmError::InvalidConfig {
                name: spec.name.clone(),
                reason: "network mode 'bridged' is not implemented yet".to_string(),
            });
        }
        NetworkMode::Cni => {
            return Err(VmmError::InvalidConfig {
                name: spec.name.clone(),
                reason: "network mode 'cni' is not implemented yet".to_string(),
            });
        }
    }

    for mount in &spec.mounts {
        let metadata = fs::metadata(&mount.host_path).map_err(|err| VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: format!(
                "failed to access shared directory {}: {err}",
                mount.host_path.display()
            ),
        })?;
        if !metadata.is_dir() {
            return Err(VmmError::InvalidConfig {
                name: spec.name.clone(),
                reason: format!(
                    "shared directory path is not a directory: {}",
                    mount.host_path.display()
                ),
            });
        }
    }

    Ok(())
}

fn validate_nested_virtualization(spec: &VmConfig) -> Result<(), VmmError> {
    if !spec.nested_virtualization {
        return Ok(());
    }

    if !GenericPlatform::is_nested_virtualization_supported() {
        return Err(VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "nested virtualization is not supported on this host".to_string(),
        });
    }

    Ok(())
}

fn validate_rosetta(spec: &VmConfig) -> Result<(), VmmError> {
    if !spec.rosetta {
        return Ok(());
    }

    match bento_vz::rosetta_availability() {
        RosettaAvailability::Installed => Ok(()),
        RosettaAvailability::NotInstalled => Err(VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "Rosetta for Linux VMs is not installed on this host. Install it with: softwareupdate --install-rosetta"
                .to_string(),
        }),
        RosettaAvailability::NotSupported => Err(VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "Rosetta is not supported on this host".to_string(),
        }),
    }
}

fn validate_machine_identifier(
    name: &str,
    machine_identifier: &MachineIdentifier,
) -> Result<(), VmmError> {
    if machine_identifier.is_empty() {
        return Ok(());
    }

    GenericMachineIdentifier::from_bytes(&machine_identifier.bytes())
        .map(|_| ())
        .map_err(|err| VmmError::InvalidConfig {
            name: name.to_string(),
            reason: err.to_string(),
        })
}

fn required_path<'a>(
    name: &str,
    path: Option<&'a PathBuf>,
    field: &'static str,
) -> Result<&'a Path, VmmError> {
    path.map(|path| path.as_path())
        .ok_or_else(|| VmmError::InvalidConfig {
            name: name.to_string(),
            reason: format!("{field} must be set"),
        })
}

async fn wait_for_state(
    events: &mut tokio::sync::watch::Receiver<VirtualMachineState>,
    vm: &VirtualMachine,
    target: VirtualMachineState,
    timeout: Duration,
) -> Result<(), VmmError> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let state = vm.state();
        tracing::debug!(current_state = %state, target_state = %target, "waiting for virtual machine state");

        if state == target {
            return Ok(());
        }

        if state == VirtualMachineState::Error {
            return Err(VmmError::Backend(format!(
                "machine entered error state while waiting for {target}"
            )));
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(VmmError::Backend(format!(
                "timed out after {:?} waiting for machine to enter {target} (current state: {state})",
                timeout
            )));
        }

        let remaining = deadline.saturating_duration_since(now);
        match tokio::time::timeout(remaining, events.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(VmmError::Backend(
                    "machine state watcher closed before target state was reached".to_string(),
                ));
            }
            Err(_) => {
                return Err(VmmError::Backend(format!(
                    "timed out after {:?} waiting for machine to enter {target} (current state: {})",
                    timeout,
                    vm.state()
                )));
            }
        }
    }
}

fn vz_error(err: bento_vz::VzError) -> VmmError {
    VmmError::Backend(err.to_string())
}
