use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
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
    VirtualMachine, VirtualMachineState,
};
use tokio::sync::{watch, Mutex as AsyncMutex};

use crate::stream::{SerialStream, VsockStream};
use crate::types::{
    MachineConfig, MachineError, MachineId, MachineState, MachineStateReceiver, NetworkMode,
    ResolvedMachineSpec,
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const BENTO_ROSETTA_TAG: &str = "bento-rosetta";

#[derive(Debug)]
pub(crate) struct VzMachineBackend {
    spec: ResolvedMachineSpec,
    inner: AsyncMutex<VzMachineState>,
    state_tx: watch::Sender<MachineState>,
}

#[derive(Debug)]
struct VzMachineState {
    vm: Option<VirtualMachine>,
    serial_port: Option<SerialPortConfiguration>,
    state: MachineState,
}

impl VzMachineBackend {
    pub(crate) fn new(spec: ResolvedMachineSpec) -> Result<Self, MachineError> {
        validate(&spec)?;
        let (state_tx, _state_rx) = watch::channel(MachineState::Created);
        Ok(Self {
            spec,
            inner: AsyncMutex::new(VzMachineState {
                vm: None,
                serial_port: None,
                state: MachineState::Created,
            }),
            state_tx,
        })
    }

    pub(crate) async fn state(&self) -> Result<MachineState, MachineError> {
        let state = self.inner.lock().await;
        match state.vm.as_ref() {
            Some(vm) => Ok(map_machine_state(vm.state())),
            None => Ok(state.state),
        }
    }

    pub(crate) async fn start(&self) -> Result<(), MachineError> {
        validate_support()?;
        let mut state = self.inner.lock().await;
        if state.vm.is_some() {
            return Err(MachineError::AlreadyRunning {
                id: self.spec.id.clone(),
            });
        }

        let (vm, serial_port) = build_vm(&self.spec)?;
        let mut state_events = vm.subscribe_state();

        vm.start().await.map_err(vz_error)?;
        wait_for_state(
            &mut state_events,
            &vm,
            VirtualMachineState::Running,
            STARTUP_TIMEOUT,
        )
        .await?;
        spawn_state_bridge(state_events, self.state_tx.clone());

        state.vm = Some(vm);
        state.serial_port = Some(serial_port);
        state.state = MachineState::Running;
        let _ = self.state_tx.send(MachineState::Running);
        Ok(())
    }

    pub(crate) async fn stop(&self) -> Result<(), MachineError> {
        let mut state = self.inner.lock().await;
        if let Some(vm) = state.vm.as_ref() {
            if vm.state() != VirtualMachineState::Stopped {
                let mut state_events = vm.subscribe_state();
                tracing::debug!(
                    machine_id = self.spec.id.as_str(),
                    current_state = %vm.state(),
                    "starting VZ shutdown flow"
                );
                let graceful_stop_completed = if vm.can_request_stop() {
                    tracing::debug!(
                        machine_id = self.spec.id.as_str(),
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
                                machine_id = self.spec.id.as_str(),
                                "graceful VZ shutdown completed"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                machine_id = self.spec.id.as_str(),
                                error = %err,
                                timeout = ?SHUTDOWN_TIMEOUT,
                                "graceful VZ shutdown did not complete before timeout, falling back to hard stop"
                            );
                        }
                    }
                    graceful_result.is_ok()
                } else {
                    tracing::debug!(
                        machine_id = self.spec.id.as_str(),
                        "guest does not support graceful request_stop, using hard stop"
                    );
                    false
                };

                if !graceful_stop_completed {
                    tracing::warn!(
                        machine_id = self.spec.id.as_str(),
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
                    tracing::debug!(machine_id = self.spec.id.as_str(), "hard VZ stop completed");
                }
            }
        }

        state.vm = None;
        state.serial_port = None;
        state.state = MachineState::Stopped;
        let _ = self.state_tx.send(MachineState::Stopped);
        Ok(())
    }

    pub(crate) fn subscribe_state(&self) -> MachineStateReceiver {
        self.state_tx.subscribe()
    }

    pub(crate) async fn open_vsock(&self, port: u32) -> Result<VsockStream, MachineError> {
        let vm = {
            let state = self.inner.lock().await;
            state.vm.clone().ok_or_else(|| {
                MachineError::Backend(format!(
                    "cannot open vsock stream because machine {:?} is not running",
                    self.spec.id.as_str()
                ))
            })?
        };

        let device = vm.open_devices().into_iter().next().ok_or_else(|| {
            MachineError::Backend("no virtio socket device configured in VM".to_string())
        })?;

        let stream = device.connect(port).await.map_err(vz_error)?;
        Ok(VsockStream::from_vz(stream))
    }

    pub(crate) async fn open_serial(&self) -> Result<SerialStream, MachineError> {
        let serial_port = {
            let state = self.inner.lock().await;
            state.serial_port.clone().ok_or_else(|| {
                MachineError::Backend(format!(
                    "cannot open serial stream because machine {:?} is not running",
                    self.spec.id.as_str()
                ))
            })?
        };

        let stream = serial_port.open_stream().map_err(vz_error)?;
        Ok(SerialStream::from_vz(stream))
    }
}

pub(crate) fn validate(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    validate_support()?;
    validate_machine_config(spec)
}

pub(crate) fn prepare(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    validate(spec)?;
    let path = machine_identifier_path(spec.id.as_str(), &spec.config)?;
    let identifier = load_or_create_machine_identifier(path)?;
    fs::write(path, identifier.data())?;
    Ok(())
}

fn validate_support() -> Result<(), MachineError> {
    let _ = VirtualMachine::builder().map_err(vz_error)?;
    Ok(())
}

fn build_vm(
    spec: &ResolvedMachineSpec,
) -> Result<(VirtualMachine, SerialPortConfiguration), MachineError> {
    let serial_port = SerialPortConfiguration::virtio_console();

    let mut builder = VirtualMachine::builder()
        .map_err(vz_error)?
        .set_cpu_count(spec.config.cpus.unwrap_or(2))
        .set_memory_size(spec.config.memory_mib.unwrap_or(2048) * 1024 * 1024)
        .set_platform(build_platform(spec)?)
        .set_boot_loader(build_boot_loader(spec)?)
        .add_entropy_device(EntropyDeviceConfiguration::new())
        .add_memory_balloon_device(MemoryBalloonDeviceConfiguration::new())
        .add_serial_port(serial_port.clone())
        .add_socket_device(SocketDeviceConfiguration::new());

    if spec.config.network == NetworkMode::VzNat {
        builder = builder.add_network_device(NetworkDeviceConfiguration::nat());
    }

    if let Some(root_disk) = spec.config.root_disk.as_ref() {
        builder = builder.add_storage_device(
            StorageDeviceConfiguration::new(root_disk.path.clone(), root_disk.read_only)
                .map_err(vz_error)?,
        );
    }

    for disk in &spec.config.data_disks {
        builder = builder.add_storage_device(
            StorageDeviceConfiguration::new(disk.path.clone(), disk.read_only).map_err(vz_error)?,
        );
    }

    for mount in &spec.config.mounts {
        let shared_dir = SharedDirectory::new(mount.host_path.clone(), mount.read_only);
        let single_share = SingleDirectoryShare::new(shared_dir);
        let mut fs_config = VirtioFileSystemDeviceConfiguration::new(mount.tag.clone());
        fs_config.set_share(single_share);
        builder = builder.add_directory_share(fs_config);
    }

    if spec.config.rosetta {
        let mut rosetta_config = VirtioFileSystemDeviceConfiguration::new(BENTO_ROSETTA_TAG);
        rosetta_config.set_rosetta_share(LinuxRosettaDirectoryShare::new().map_err(vz_error)?);
        builder = builder.add_directory_share(rosetta_config);
    }

    let vm = builder.build().map_err(vz_error)?;
    Ok((vm, serial_port))
}

fn build_platform(spec: &ResolvedMachineSpec) -> Result<GenericPlatform, MachineError> {
    let path = machine_identifier_path(spec.id.as_str(), &spec.config)?;
    let mut platform = GenericPlatform::new();
    let machine_identifier = load_or_create_machine_identifier(path)?;
    platform.set_machine_identifier(machine_identifier);
    platform.set_nested_virtualization_enabled(spec.config.nested_virtualization);
    Ok(platform)
}

fn build_boot_loader(spec: &ResolvedMachineSpec) -> Result<LinuxBootLoader, MachineError> {
    let kernel_path = required_path(&spec.id, spec.config.kernel_path.as_ref(), "kernel_path")?;
    let initramfs_path = required_path(
        &spec.id,
        spec.config.initramfs_path.as_ref(),
        "initramfs_path",
    )?;

    let mut boot_loader = LinuxBootLoader::new(kernel_path);
    boot_loader.set_initial_ramdisk(initramfs_path);

    let root_arg = spec
        .config
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

fn load_or_create_machine_identifier(
    path: &Path,
) -> Result<GenericMachineIdentifier, MachineError> {
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(GenericMachineIdentifier::new()),
        Ok(bytes) => GenericMachineIdentifier::from_bytes(&bytes).map_err(vz_error),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(GenericMachineIdentifier::new()),
        Err(err) => Err(MachineError::Backend(format!(
            "read machine identifier file {}: {err}",
            path.display()
        ))),
    }
}

fn validate_machine_config(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    if spec.config.machine_directory.as_os_str().is_empty() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "machine_directory must be set".to_string(),
        });
    }

    if spec.config.cpus == Some(0) {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "cpu count must be greater than zero".to_string(),
        });
    }

    if spec.config.memory_mib == Some(0) {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "memory_mib must be greater than zero".to_string(),
        });
    }

    let _ = required_path(&spec.id, spec.config.kernel_path.as_ref(), "kernel_path")?;
    let _ = required_path(
        &spec.id,
        spec.config.initramfs_path.as_ref(),
        "initramfs_path",
    )?;
    let _ = machine_identifier_path(spec.id.as_str(), &spec.config)?;

    validate_nested_virtualization(spec)?;
    validate_rosetta(spec)?;

    match spec.config.network {
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

    for mount in &spec.config.mounts {
        let metadata =
            fs::metadata(&mount.host_path).map_err(|err| MachineError::InvalidConfig {
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

fn validate_nested_virtualization(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    if !spec.config.nested_virtualization {
        return Ok(());
    }

    if !GenericPlatform::is_nested_virtualization_supported() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "nested virtualization is not supported on this host".to_string(),
        });
    }

    Ok(())
}

fn validate_rosetta(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    if !spec.config.rosetta {
        return Ok(());
    }

    match bento_vz::rosetta_availability() {
        RosettaAvailability::Installed => Ok(()),
        RosettaAvailability::NotInstalled => Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "Rosetta for Linux VMs is not installed on this host. Install it with: softwareupdate --install-rosetta"
                .to_string(),
        }),
        RosettaAvailability::NotSupported => Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "Rosetta is not supported on this host".to_string(),
        }),
    }
}

fn machine_identifier_path<'a>(
    id: &str,
    config: &'a MachineConfig,
) -> Result<&'a Path, MachineError> {
    config
        .machine_identifier_path
        .as_deref()
        .ok_or_else(|| MachineError::InvalidConfig {
            id: MachineId::from(id),
            reason: "machine_identifier_path must be set for VZ".to_string(),
        })
}

fn required_path<'a>(
    id: &MachineId,
    path: Option<&'a PathBuf>,
    field: &'static str,
) -> Result<&'a Path, MachineError> {
    path.map(|path| path.as_path())
        .ok_or_else(|| MachineError::InvalidConfig {
            id: id.clone(),
            reason: format!("{field} must be set"),
        })
}

fn spawn_state_bridge(
    mut events: tokio::sync::watch::Receiver<VirtualMachineState>,
    state_tx: watch::Sender<MachineState>,
) {
    tokio::spawn(async move {
        loop {
            if events.changed().await.is_err() {
                let _ = state_tx.send(MachineState::Stopped);
                return;
            }

            let state = *events.borrow_and_update();
            match state {
                VirtualMachineState::Stopped => {
                    let _ = state_tx.send(MachineState::Stopped);
                    return;
                }
                VirtualMachineState::Error => {
                    let _ = state_tx.send(MachineState::Stopped);
                    return;
                }
                VirtualMachineState::Running => {
                    let _ = state_tx.send(MachineState::Running);
                }
                _ => {
                    let _ = state_tx.send(MachineState::Created);
                }
            }
        }
    });
}

async fn wait_for_state(
    events: &mut tokio::sync::watch::Receiver<VirtualMachineState>,
    vm: &VirtualMachine,
    target: VirtualMachineState,
    timeout: Duration,
) -> Result<(), MachineError> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let state = vm.state();
        tracing::debug!(current_state = %state, target_state = %target, "waiting for virtual machine state");

        if state == target {
            return Ok(());
        }

        if state == VirtualMachineState::Error {
            return Err(MachineError::Backend(format!(
                "machine entered error state while waiting for {target}"
            )));
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(MachineError::Backend(format!(
                "timed out after {:?} waiting for machine to enter {target} (current state: {state})",
                timeout
            )));
        }

        let remaining = deadline.saturating_duration_since(now);
        match tokio::time::timeout(remaining, events.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return Err(MachineError::Backend(
                    "machine state watcher closed before target state was reached".to_string(),
                ));
            }
            Err(_) => {
                return Err(MachineError::Backend(format!(
                    "timed out after {:?} waiting for machine to enter {target} (current state: {})",
                    timeout,
                    vm.state()
                )));
            }
        }
    }
}

fn vz_error(err: bento_vz::VzError) -> MachineError {
    MachineError::Backend(err.to_string())
}

fn map_machine_state(state: VirtualMachineState) -> MachineState {
    match state {
        VirtualMachineState::Running => MachineState::Running,
        VirtualMachineState::Stopped => MachineState::Stopped,
        _ => MachineState::Created,
    }
}
