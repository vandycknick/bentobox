use std::env;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bento_fc::types::{
    BootSource, Drive, DriveCacheType, DriveIoEngine, MachineConfiguration, Vsock,
};
use bento_fc::FirecrackerProcessBuilder;
use bento_protocol::{DEFAULT_AGENT_PORT, KERNEL_PARAM_AGENT_PORT};
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::time::timeout;

use crate::stream::{SerialStream, VsockStream};
use crate::types::{
    DiskImage, MachineError, MachineKind, MachineState, MachineStateReceiver, NetworkMode,
    ResolvedMachineSpec,
};

const FIRECRACKER_BINARY_ENV: &str = "FIRECRACKER_BIN";
const FIRECRACKER_BINARY_NAME: &str = "firecracker";
const API_SOCKET_NAME: &str = "firecracker.sock";
const TRACE_LOG_NAME: &str = "fc.trace.log";
const VSOCK_SOCKET_NAME: &str = "firecracker.vsock";
const GUEST_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct FirecrackerMachineBackend {
    spec: ResolvedMachineSpec,
    firecracker_bin: PathBuf,
    runtime_dir: PathBuf,
    api_socket_path: PathBuf,
    trace_log_path: PathBuf,
    vsock_socket_path: PathBuf,
    state: Arc<Mutex<MachineState>>,
    runtime: AsyncMutex<Option<RunningFirecracker>>,
    state_tx: watch::Sender<MachineState>,
}

struct RunningFirecracker {
    process: Arc<bento_fc::FirecrackerProcess>,
    vm: bento_fc::VirtualMachine,
    exit_watcher: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for FirecrackerMachineBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrackerMachineBackend")
            .field("id", &self.spec.id.as_str())
            .field("runtime_dir", &self.runtime_dir)
            .finish_non_exhaustive()
    }
}

pub(crate) fn validate(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    let config = &spec.config;
    if config.cpus.is_none() {
        return invalid_config(spec, "firecracker requires a CPU count");
    }
    if config.memory_mib.is_none() {
        return invalid_config(spec, "firecracker requires a memory size");
    }
    if config.kernel_path.is_none() {
        return invalid_config(spec, "firecracker requires a kernel image path");
    }
    if config.initramfs_path.is_none() {
        return invalid_config(spec, "firecracker requires an initramfs path");
    }
    if matches!(config.cpus, Some(0)) {
        return invalid_config(spec, "firecracker requires at least one vCPU");
    }
    if matches!(config.memory_mib, Some(0)) {
        return invalid_config(
            spec,
            "firecracker requires memory_mib to be greater than zero",
        );
    }
    if !config.mounts.is_empty() {
        return invalid_config(
            spec,
            "shared directory mounts are not implemented for the firecracker backend yet",
        );
    }
    if config.machine_identifier_path.is_some() {
        return invalid_config(
            spec,
            "machine identifiers are not used by the firecracker backend",
        );
    }
    if config.nested_virtualization {
        return invalid_config(
            spec,
            "nested virtualization is not implemented for the firecracker backend yet",
        );
    }
    if config.rosetta {
        return invalid_config(
            spec,
            "rosetta is not implemented for the firecracker backend",
        );
    }

    match config.network {
        NetworkMode::None => {}
        NetworkMode::VzNat => {
            return invalid_config(spec, "vznat networking is only supported by the VZ backend");
        }
        NetworkMode::Bridged => {
            return invalid_config(
                spec,
                "bridged networking is not implemented for the firecracker backend yet",
            );
        }
        NetworkMode::Cni => {
            return invalid_config(
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
        return invalid_config(spec, "machine_directory must be set");
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

impl FirecrackerMachineBackend {
    pub(crate) fn new(spec: ResolvedMachineSpec) -> Result<Self, MachineError> {
        validate(&spec)?;
        let firecracker_bin = locate_firecracker_binary()?;
        let runtime_dir = runtime_dir_for(&spec);
        let api_socket_path = runtime_dir.join(API_SOCKET_NAME);
        let trace_log_path = runtime_dir.join(TRACE_LOG_NAME);
        let vsock_socket_path = runtime_dir.join(VSOCK_SOCKET_NAME);
        let (state_tx, _state_rx) = watch::channel(MachineState::Created);

        Ok(Self {
            spec,
            firecracker_bin,
            runtime_dir,
            api_socket_path,
            trace_log_path,
            vsock_socket_path,
            state: Arc::new(Mutex::new(MachineState::Created)),
            runtime: AsyncMutex::new(None),
            state_tx,
        })
    }

    pub(crate) async fn state(&self) -> Result<MachineState, MachineError> {
        let mut runtime = self.runtime.lock().await;
        self.refresh_state_from_process_locked(&mut runtime)?;
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)
    }

    pub(crate) async fn start(&self) -> Result<(), MachineError> {
        let mut runtime = self.runtime.lock().await;
        self.refresh_state_from_process_locked(&mut runtime)?;
        if self
            .state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)?
            == MachineState::Running
        {
            return Err(MachineError::AlreadyRunning {
                id: self.spec.id.clone(),
            });
        }
        if runtime.is_some() {
            return Err(MachineError::Backend(
                "firecracker restart is not implemented yet".to_string(),
            ));
        }

        prepare(&self.spec)?;
        self.ensure_runtime_dir()?;

        let process = Arc::new(
            FirecrackerProcessBuilder::new(&self.firecracker_bin, &self.api_socket_path)
                .id(self.spec.id.as_str())
                .log_path(&self.trace_log_path)
                .log_level("Info")
                .spawn()
                .await
                .map_err(fc_error)?,
        );

        let vm = process
            .builder()
            .boot_source(build_boot_source(&self.spec)?)
            .machine_config(build_machine_configuration(&self.spec)?)
            .vsock(build_vsock(&self.spec, &self.vsock_socket_path))
            .add_drive_if_some(build_root_drive(&self.spec)?)
            .add_drives(build_data_drives(&self.spec)?)
            .start()
            .await
            .map_err(fc_error)?;

        let exit_watcher = spawn_exit_watcher(
            self.spec.id.as_str().to_string(),
            process.clone(),
            self.state.clone(),
            self.state_tx.clone(),
        );

        *runtime = Some(RunningFirecracker {
            process,
            vm,
            exit_watcher: Some(exit_watcher),
        });
        self.set_state(MachineState::Running)?;
        let _ = self.state_tx.send(MachineState::Running);
        Ok(())
    }

    pub(crate) async fn stop(&self) -> Result<(), MachineError> {
        let running = {
            let mut runtime = self.runtime.lock().await;
            runtime.take()
        };

        let Some(mut running) = running else {
            self.set_state(MachineState::Stopped)?;
            return Ok(());
        };

        if running.process.try_wait().map_err(fc_error)?.is_none() {
            let graceful_shutdown = running.vm.send_ctrl_alt_del().await;
            match graceful_shutdown {
                Ok(()) => {
                    tracing::debug!(
                        machine_id = self.spec.id.as_str(),
                        timeout = ?GUEST_SHUTDOWN_TIMEOUT,
                        "sent Ctrl+Alt+Del to guest, waiting for graceful shutdown"
                    );
                    match timeout(GUEST_SHUTDOWN_TIMEOUT, running.process.wait()).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => return Err(fc_error(err)),
                        Err(_) => {
                            tracing::warn!(
                                machine_id = self.spec.id.as_str(),
                                timeout = ?GUEST_SHUTDOWN_TIMEOUT,
                                "guest did not shut down after Ctrl+Alt+Del, falling back to SIGTERM"
                            );
                            shutdown_process(&self.spec, &running.process).await?;
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        machine_id = self.spec.id.as_str(),
                        error = %err,
                        "failed to send Ctrl+Alt+Del, falling back to SIGTERM"
                    );
                    shutdown_process(&self.spec, &running.process).await?;
                }
            }
        }

        if let Some(exit_watcher) = running.exit_watcher.take() {
            exit_watcher.join().map_err(|_| {
                MachineError::Backend("firecracker exit watcher panicked".to_string())
            })?;
        }

        self.set_state(MachineState::Stopped)?;
        let _ = self.state_tx.send(MachineState::Stopped);
        if self.api_socket_path.exists() {
            let _ = std::fs::remove_file(&self.api_socket_path);
        }
        if self.vsock_socket_path.exists() {
            let _ = std::fs::remove_file(&self.vsock_socket_path);
        }
        Ok(())
    }

    pub(crate) async fn open_vsock(&self, port: u32) -> Result<VsockStream, MachineError> {
        let vsock = {
            let mut runtime = self.runtime.lock().await;
            self.refresh_state_from_process_locked(&mut runtime)?;
            self.ensure_running_state()?;
            let running = runtime.as_ref().ok_or_else(|| not_running(&self.spec))?;
            running.vm.vsock().map_err(fc_error)?
        };

        let stream = vsock.connect(port).await.map_err(fc_error)?;
        Ok(VsockStream::from_firecracker(stream))
    }

    pub(crate) async fn open_serial(&self) -> Result<SerialStream, MachineError> {
        let mut runtime = self.runtime.lock().await;
        self.refresh_state_from_process_locked(&mut runtime)?;
        self.ensure_running_state()?;
        let running = runtime.as_ref().ok_or_else(|| not_running(&self.spec))?;
        Ok(SerialStream::from_firecracker(
            running.process.serial().map_err(fc_error)?,
        ))
    }

    pub(crate) fn subscribe_state(&self) -> MachineStateReceiver {
        self.state_tx.subscribe()
    }

    fn set_state(&self, state: MachineState) -> Result<(), MachineError> {
        let mut current = self
            .state
            .lock()
            .map_err(|_| MachineError::RegistryPoisoned)?;
        *current = state;
        Ok(())
    }

    fn ensure_runtime_dir(&self) -> Result<(), MachineError> {
        std::fs::create_dir_all(&self.runtime_dir)?;
        if self.api_socket_path.exists() {
            std::fs::remove_file(&self.api_socket_path)?;
        }
        if self.vsock_socket_path.exists() {
            let _ = std::fs::remove_file(&self.vsock_socket_path);
        }
        if self.trace_log_path.exists() {
            let _ = std::fs::remove_file(&self.trace_log_path);
        }
        Ok(())
    }

    fn ensure_running_state(&self) -> Result<(), MachineError> {
        let state = self
            .state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)?;
        if state == MachineState::Running {
            return Ok(());
        }
        Err(not_running(&self.spec))
    }

    fn refresh_state_from_process_locked(
        &self,
        runtime: &mut Option<RunningFirecracker>,
    ) -> Result<(), MachineError> {
        let Some(running) = runtime.as_ref() else {
            return Ok(());
        };

        if running.process.try_wait().map_err(fc_error)?.is_some() {
            self.set_state(MachineState::Stopped)?;
            let _ = self.state_tx.send(MachineState::Stopped);
        }

        Ok(())
    }
}

trait VirtualMachineBuilderExt {
    fn add_drive_if_some(self, drive: Option<Drive>) -> Self;
    fn add_drives(self, drives: Vec<Drive>) -> Self;
}

impl VirtualMachineBuilderExt for bento_fc::VirtualMachineBuilder {
    fn add_drive_if_some(self, drive: Option<Drive>) -> Self {
        match drive {
            Some(drive) => self.add_drive(drive),
            None => self,
        }
    }

    fn add_drives(self, drives: Vec<Drive>) -> Self {
        drives
            .into_iter()
            .fold(self, |builder, drive| builder.add_drive(drive))
    }
}

fn spawn_exit_watcher(
    machine_id: String,
    process: Arc<bento_fc::FirecrackerProcess>,
    state: Arc<Mutex<MachineState>>,
    state_tx: watch::Sender<MachineState>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("firecracker-machine-state:{machine_id}"))
        .spawn(move || loop {
            match process.try_wait() {
                Ok(Some(status)) => {
                    if let Ok(mut current_state) = state.lock() {
                        *current_state = MachineState::Stopped;
                    }
                    let _ = state_tx.send(MachineState::Stopped);
                    tracing::info!(machine_id, status = %status, "firecracker process exited");
                    return;
                }
                Ok(None) => thread::sleep(EXIT_POLL_INTERVAL),
                Err(err) => {
                    if let Ok(mut current_state) = state.lock() {
                        *current_state = MachineState::Stopped;
                    }
                    let _ = state_tx.send(MachineState::Stopped);
                    tracing::warn!(machine_id, error = %err, "failed to poll firecracker process status");
                    return;
                }
            }
        })
        .expect("firecracker exit watcher thread should spawn")
}

fn validate_support() -> Result<(), MachineError> {
    locate_firecracker_binary()?;
    if !Path::new("/dev/kvm").exists() {
        return Err(MachineError::UnsupportedBackend {
            kind: MachineKind::Firecracker,
            reason: "firecracker requires /dev/kvm on Linux hosts".to_string(),
        });
    }
    Ok(())
}

fn locate_firecracker_binary() -> Result<PathBuf, MachineError> {
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

fn runtime_dir_for(spec: &ResolvedMachineSpec) -> PathBuf {
    spec.config.machine_directory.clone()
}

fn ensure_path_exists(
    spec: &ResolvedMachineSpec,
    path: &Path,
    label: &str,
) -> Result<(), MachineError> {
    if path.exists() {
        return Ok(());
    }

    invalid_config(spec, &format!("{label} does not exist: {}", path.display()))
}

fn build_boot_args(config: &crate::types::MachineConfig) -> String {
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
        KERNEL_PARAM_AGENT_PORT, DEFAULT_AGENT_PORT
    ));
    args.join(" ")
}

fn guest_cid_for(spec: &ResolvedMachineSpec) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.id.as_str().hash(&mut hasher);
    3u32 + (hasher.finish() as u32 % 0x3fff_fffc)
}

fn build_boot_source(spec: &ResolvedMachineSpec) -> Result<BootSource, MachineError> {
    Ok(BootSource {
        kernel_image_path: spec
            .config
            .kernel_path
            .as_ref()
            .expect("validated kernel path missing")
            .display()
            .to_string(),
        boot_args: Some(build_boot_args(&spec.config)),
        initrd_path: Some(
            spec.config
                .initramfs_path
                .as_ref()
                .expect("validated initramfs path missing")
                .display()
                .to_string(),
        ),
    })
}

fn build_machine_configuration(
    spec: &ResolvedMachineSpec,
) -> Result<MachineConfiguration, MachineError> {
    let vcpu_count =
        u64::try_from(spec.config.cpus.expect("validated cpus missing")).map_err(|_| {
            MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: "vCPU count does not fit in u64".to_string(),
            }
        })?;
    let vcpu_count = NonZeroU64::new(vcpu_count).ok_or_else(|| MachineError::InvalidConfig {
        id: spec.id.clone(),
        reason: "firecracker requires at least one vCPU".to_string(),
    })?;

    Ok(MachineConfiguration {
        vcpu_count,
        mem_size_mib: i64::try_from(spec.config.memory_mib.expect("validated memory missing"))
            .map_err(|_| MachineError::InvalidConfig {
                id: spec.id.clone(),
                reason: "memory_mib does not fit in i64".to_string(),
            })?,
        smt: false,
        track_dirty_pages: false,
        cpu_template: None,
        huge_pages: None,
    })
}

fn build_root_drive(spec: &ResolvedMachineSpec) -> Result<Option<Drive>, MachineError> {
    spec.config
        .root_disk
        .as_ref()
        .map(|disk| build_drive("rootfs".to_string(), disk, true))
        .transpose()
}

fn build_data_drives(spec: &ResolvedMachineSpec) -> Result<Vec<Drive>, MachineError> {
    spec.config
        .data_disks
        .iter()
        .enumerate()
        .map(|(index, disk)| build_drive(format!("disk{}", index + 1), disk, false))
        .collect()
}

fn build_drive(
    drive_id: String,
    disk: &DiskImage,
    is_root_device: bool,
) -> Result<Drive, MachineError> {
    Ok(Drive {
        drive_id,
        path_on_host: Some(disk.path.display().to_string()),
        is_root_device,
        partuuid: None,
        is_read_only: Some(disk.read_only),
        cache_type: DriveCacheType::Unsafe,
        io_engine: DriveIoEngine::Sync,
        rate_limiter: None,
        socket: None,
    })
}

fn build_vsock(spec: &ResolvedMachineSpec, socket_path: &PathBuf) -> Vsock {
    Vsock {
        guest_cid: guest_cid_for(spec) as i64,
        uds_path: socket_path.display().to_string(),
        vsock_id: None,
    }
}

fn not_running(spec: &ResolvedMachineSpec) -> MachineError {
    MachineError::Backend(format!("machine {:?} is not running", spec.id.as_str()))
}

fn fc_error(error: bento_fc::FirecrackerError) -> MachineError {
    MachineError::Backend(error.to_string())
}

async fn shutdown_process(
    spec: &ResolvedMachineSpec,
    process: &bento_fc::FirecrackerProcess,
) -> Result<(), MachineError> {
    let shutdown_result = timeout(STOP_TIMEOUT, process.shutdown()).await;
    match shutdown_result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(fc_error(err)),
        Err(_) => {
            tracing::warn!(
                machine_id = spec.id.as_str(),
                timeout = ?STOP_TIMEOUT,
                "firecracker did not stop after SIGTERM, sending SIGKILL"
            );
            process.kill().await.map_err(fc_error)?;
            Ok(())
        }
    }
}

fn invalid_config<T>(spec: &ResolvedMachineSpec, reason: &str) -> Result<T, MachineError> {
    Err(MachineError::InvalidConfig {
        id: spec.id.clone(),
        reason: reason.to_string(),
    })
}
