use std::env;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bento_fc::types::{
    BootSource, Drive, DriveCacheType, DriveIoEngine, MachineConfiguration, Vsock,
};
use bento_fc::FirecrackerProcessBuilder;
use bento_protocol::{DEFAULT_DISCOVERY_PORT, KERNEL_PARAM_DISCOVERY_PORT};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

use crate::stream::{MachineSerialStream, VsockStream};
use crate::types::{Backend, DiskImage, NetworkMode, VmConfig, VmExit, VmmError};

const FIRECRACKER_BINARY_ENV: &str = "FIRECRACKER_BIN";
const FIRECRACKER_BINARY_NAME: &str = "firecracker";
const API_SOCKET_NAME: &str = "firecracker.sock";
const TRACE_LOG_NAME: &str = "fc.trace.log";
const VSOCK_SOCKET_NAME: &str = "firecracker.vsock";
const GUEST_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct FirecrackerMachineBackend {
    config: VmConfig,
    firecracker_bin: PathBuf,
    runtime_dir: PathBuf,
    api_socket_path: PathBuf,
    trace_log_path: PathBuf,
    vsock_socket_path: PathBuf,
    exit: Arc<Mutex<Option<VmExit>>>,
    runtime: AsyncMutex<Option<RunningFirecracker>>,
}

struct RunningFirecracker {
    process: Arc<bento_fc::FirecrackerProcess>,
    vm: bento_fc::VirtualMachine,
}

impl std::fmt::Debug for FirecrackerMachineBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrackerMachineBackend")
            .field("name", &self.config.name.as_str())
            .field("runtime_dir", &self.runtime_dir)
            .finish_non_exhaustive()
    }
}

pub(crate) fn validate(config: &VmConfig) -> Result<(), VmmError> {
    let machine = config;
    if machine.cpus.is_none() {
        return invalid_config(config, "firecracker requires a CPU count");
    }
    if machine.memory_mib.is_none() {
        return invalid_config(config, "firecracker requires a memory size");
    }
    if machine.kernel_path.is_none() {
        return invalid_config(config, "firecracker requires a kernel image path");
    }
    if machine.initramfs_path.is_none() {
        return invalid_config(config, "firecracker requires an initramfs path");
    }
    if matches!(machine.cpus, Some(0)) {
        return invalid_config(config, "firecracker requires at least one vCPU");
    }
    if matches!(machine.memory_mib, Some(0)) {
        return invalid_config(
            config,
            "firecracker requires memory_mib to be greater than zero",
        );
    }
    if !machine.mounts.is_empty() {
        return invalid_config(
            config,
            "shared directory mounts are not implemented for the firecracker backend yet",
        );
    }
    if machine.machine_identifier.is_some() {
        return invalid_config(
            config,
            "machine identifiers are not used by the firecracker backend",
        );
    }
    if machine.nested_virtualization {
        return invalid_config(
            config,
            "nested virtualization is not implemented for the firecracker backend yet",
        );
    }
    if machine.rosetta {
        return invalid_config(
            config,
            "rosetta is not implemented for the firecracker backend",
        );
    }

    match machine.network {
        NetworkMode::None => {}
        NetworkMode::VzNat => {
            return invalid_config(
                config,
                "vznat networking is only supported by the VZ backend",
            );
        }
        NetworkMode::Bridged => {
            return invalid_config(
                config,
                "bridged networking is not implemented for the firecracker backend yet",
            );
        }
        NetworkMode::Cni => {
            return invalid_config(
                config,
                "cni networking is not implemented for the firecracker backend yet",
            );
        }
    }

    Ok(())
}

fn prepare(config: &VmConfig) -> Result<(), VmmError> {
    validate(config)?;
    validate_support()?;

    let kernel_path = config
        .kernel_path
        .as_ref()
        .expect("validated kernel path missing");
    let initramfs_path = config
        .initramfs_path
        .as_ref()
        .expect("validated initramfs path missing");

    if config.base_directory.as_os_str().is_empty() {
        return invalid_config(config, "base_directory must be set");
    }

    ensure_path_exists(config, kernel_path, "kernel image")?;
    ensure_path_exists(config, initramfs_path, "initramfs")?;
    if let Some(root_disk) = config.root_disk.as_ref() {
        ensure_path_exists(config, &root_disk.path, "root disk")?;
    }
    for (index, disk) in config.data_disks.iter().enumerate() {
        ensure_path_exists(config, &disk.path, &format!("data disk #{index}"))?;
    }

    std::fs::create_dir_all(runtime_dir_for(config))?;
    Ok(())
}

impl FirecrackerMachineBackend {
    pub(crate) fn new(config: VmConfig) -> Result<Self, VmmError> {
        validate(&config)?;
        let firecracker_bin = locate_firecracker_binary()?;
        let runtime_dir = runtime_dir_for(&config);
        let api_socket_path = runtime_dir.join(API_SOCKET_NAME);
        let trace_log_path = runtime_dir.join(TRACE_LOG_NAME);
        let vsock_socket_path = runtime_dir.join(VSOCK_SOCKET_NAME);

        Ok(Self {
            config,
            firecracker_bin,
            runtime_dir,
            api_socket_path,
            trace_log_path,
            vsock_socket_path,
            exit: Arc::new(Mutex::new(None)),
            runtime: AsyncMutex::new(None),
        })
    }

    pub(crate) async fn start(&self) -> Result<(), VmmError> {
        let mut runtime = self.runtime.lock().await;
        if runtime.is_some() {
            return Err(VmmError::AlreadyRunning {
                name: self.config.name.clone(),
            });
        }

        prepare(&self.config)?;
        self.ensure_runtime_dir()?;
        self.clear_exit_cache()?;

        let process = Arc::new(
            FirecrackerProcessBuilder::new(&self.firecracker_bin, &self.api_socket_path)
                .id(self.config.name.as_str())
                .log_path(&self.trace_log_path)
                .log_level("Info")
                .spawn()
                .await
                .map_err(fc_error)?,
        );

        let vm = process
            .builder()
            .boot_source(build_boot_source(&self.config)?)
            .machine_config(build_machine_configuration(&self.config)?)
            .vsock(build_vsock(&self.config, &self.vsock_socket_path))
            .add_drive_if_some(build_root_drive(&self.config)?)
            .add_drives(build_data_drives(&self.config)?)
            .start()
            .await
            .map_err(fc_error)?;

        *runtime = Some(RunningFirecracker { process, vm });
        Ok(())
    }

    pub(crate) async fn stop(&self) -> Result<(), VmmError> {
        let running = {
            let mut runtime = self.runtime.lock().await;
            runtime.take()
        };

        let Some(running) = running else {
            self.cache_exit(VmExit::Stopped)?;
            return Ok(());
        };

        if running.process.try_wait().map_err(fc_error)?.is_none() {
            let graceful_shutdown = running.vm.send_ctrl_alt_del().await;
            match graceful_shutdown {
                Ok(()) => {
                    tracing::debug!(
                        machine_id = self.config.name.as_str(),
                        timeout = ?GUEST_SHUTDOWN_TIMEOUT,
                        "sent Ctrl+Alt+Del to guest, waiting for graceful shutdown"
                    );
                    match timeout(GUEST_SHUTDOWN_TIMEOUT, running.process.wait()).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => return Err(fc_error(err)),
                        Err(_) => {
                            tracing::warn!(
                                machine_id = self.config.name.as_str(),
                                timeout = ?GUEST_SHUTDOWN_TIMEOUT,
                                "guest did not shut down after Ctrl+Alt+Del, falling back to SIGTERM"
                            );
                            shutdown_process(&self.config, &running.process).await?;
                        }
                    }
                }
                Err(err) => {
                    let fault_message = err.fault_message();
                    let status = err.status().map(|s| s.as_u16()).unwrap_or_default();
                    let operation = err.operation();
                    tracing::warn!(
                        machine_id = self.config.name.as_str(),
                        status = status,
                        operation,
                        message = fault_message,
                        "failed to send Ctrl+Alt+Del, falling back to SIGTERM"
                    );
                    shutdown_process(&self.config, &running.process).await?;
                }
            }
        }

        self.cache_exit(VmExit::Stopped)?;
        self.cleanup_runtime_files();
        Ok(())
    }

    pub(crate) async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmmError> {
        let vsock = {
            let runtime = self.runtime.lock().await;
            let running = runtime.as_ref().ok_or_else(|| not_running(&self.config))?;
            running.vm.vsock().map_err(fc_error)?
        };

        let stream = vsock.connect(port).await.map_err(fc_error)?;
        Ok(VsockStream::from_firecracker(stream))
    }

    pub(crate) async fn open_serial(&self) -> Result<MachineSerialStream, VmmError> {
        let runtime = self.runtime.lock().await;
        let running = runtime.as_ref().ok_or_else(|| not_running(&self.config))?;
        Ok(MachineSerialStream::from_firecracker(
            running.process.serial().map_err(fc_error)?,
        ))
    }

    pub(crate) async fn wait(&self) -> Result<VmExit, VmmError> {
        if let Some(exit) = self.cached_exit()? {
            return Ok(exit);
        }

        let process = {
            let runtime = self.runtime.lock().await;
            let Some(running) = runtime.as_ref() else {
                return Err(VmmError::Backend(
                    "cannot wait for a virtual machine that has not been started".to_string(),
                ));
            };
            Arc::clone(&running.process)
        };

        let status = process.wait().await.map_err(fc_error)?;
        let exit = vm_exit_from_status(status);
        self.cache_exit(exit.clone())?;
        let mut runtime = self.runtime.lock().await;
        *runtime = None;
        self.cleanup_runtime_files();
        Ok(exit)
    }

    pub(crate) async fn try_wait(&self) -> Result<Option<VmExit>, VmmError> {
        if let Some(exit) = self.cached_exit()? {
            return Ok(Some(exit));
        }

        let maybe_status = {
            let runtime = self.runtime.lock().await;
            let Some(running) = runtime.as_ref() else {
                return Ok(None);
            };
            running.process.try_wait().map_err(fc_error)?
        };

        let Some(status) = maybe_status else {
            return Ok(None);
        };

        let exit = vm_exit_from_status(status);
        self.cache_exit(exit.clone())?;
        let mut runtime = self.runtime.lock().await;
        *runtime = None;
        self.cleanup_runtime_files();
        Ok(Some(exit))
    }

    fn ensure_runtime_dir(&self) -> Result<(), VmmError> {
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
        }
        Ok(())
    }

    fn clear_exit_cache(&self) -> Result<(), VmmError> {
        let mut slot = self.exit.lock().map_err(|_| VmmError::RegistryPoisoned)?;
        *slot = None;
        Ok(())
    }

    fn cleanup_runtime_files(&self) {
        if self.api_socket_path.exists() {
            let _ = std::fs::remove_file(&self.api_socket_path);
        }
        if self.vsock_socket_path.exists() {
            let _ = std::fs::remove_file(&self.vsock_socket_path);
        }
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

fn validate_support() -> Result<(), VmmError> {
    locate_firecracker_binary()?;
    if !Path::new("/dev/kvm").exists() {
        return Err(VmmError::UnsupportedBackend {
            kind: Backend::Firecracker,
            reason: "firecracker requires /dev/kvm on Linux hosts".to_string(),
        });
    }
    Ok(())
}

fn locate_firecracker_binary() -> Result<PathBuf, VmmError> {
    if let Some(path) = env::var_os(FIRECRACKER_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }

        return Err(VmmError::UnsupportedBackend {
            kind: Backend::Firecracker,
            reason: format!(
                "{FIRECRACKER_BINARY_ENV} is set but does not point to a file: {}",
                path.display()
            ),
        });
    }

    let path = env::var_os("PATH").ok_or_else(|| VmmError::UnsupportedBackend {
        kind: Backend::Firecracker,
        reason: "PATH is not set, so the firecracker binary cannot be located".to_string(),
    })?;

    for entry in env::split_paths(&path) {
        let candidate = entry.join(FIRECRACKER_BINARY_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(VmmError::UnsupportedBackend {
        kind: Backend::Firecracker,
        reason: "firecracker binary was not found in PATH".to_string(),
    })
}

fn runtime_dir_for(spec: &VmConfig) -> PathBuf {
    spec.base_directory.clone()
}

fn ensure_path_exists(config: &VmConfig, path: &Path, label: &str) -> Result<(), VmmError> {
    if path.exists() {
        return Ok(());
    }

    invalid_config(
        config,
        &format!("{label} does not exist: {}", path.display()),
    )
}

fn build_boot_args(config: &crate::types::VmConfig) -> String {
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
        KERNEL_PARAM_DISCOVERY_PORT, DEFAULT_DISCOVERY_PORT
    ));
    args.join(" ")
}

fn guest_cid_for(spec: &VmConfig) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.name.as_str().hash(&mut hasher);
    3u32 + (hasher.finish() as u32 % 0x3fff_fffc)
}

fn build_boot_source(spec: &VmConfig) -> Result<BootSource, VmmError> {
    Ok(BootSource {
        kernel_image_path: spec
            .kernel_path
            .as_ref()
            .expect("validated kernel path missing")
            .display()
            .to_string(),
        boot_args: Some(build_boot_args(spec)),
        initrd_path: Some(
            spec.initramfs_path
                .as_ref()
                .expect("validated initramfs path missing")
                .display()
                .to_string(),
        ),
    })
}

fn build_machine_configuration(spec: &VmConfig) -> Result<MachineConfiguration, VmmError> {
    let vcpu_count = u64::try_from(spec.cpus.expect("validated cpus missing")).map_err(|_| {
        VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "vCPU count does not fit in u64".to_string(),
        }
    })?;
    let vcpu_count = NonZeroU64::new(vcpu_count).ok_or_else(|| VmmError::InvalidConfig {
        name: spec.name.clone(),
        reason: "firecracker requires at least one vCPU".to_string(),
    })?;

    Ok(MachineConfiguration {
        vcpu_count,
        mem_size_mib: i64::try_from(spec.memory_mib.expect("validated memory missing")).map_err(
            |_| VmmError::InvalidConfig {
                name: spec.name.clone(),
                reason: "memory_mib does not fit in i64".to_string(),
            },
        )?,
        smt: false,
        track_dirty_pages: false,
        cpu_template: None,
        huge_pages: None,
    })
}

fn build_root_drive(spec: &VmConfig) -> Result<Option<Drive>, VmmError> {
    spec.root_disk
        .as_ref()
        .map(|disk| build_drive("rootfs".to_string(), disk, true))
        .transpose()
}

fn build_data_drives(spec: &VmConfig) -> Result<Vec<Drive>, VmmError> {
    spec.data_disks
        .iter()
        .enumerate()
        .map(|(index, disk)| build_drive(format!("disk{}", index + 1), disk, false))
        .collect()
}

fn build_drive(
    drive_name: String,
    disk: &DiskImage,
    is_root_device: bool,
) -> Result<Drive, VmmError> {
    Ok(Drive {
        drive_id: drive_name,
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

fn build_vsock(spec: &VmConfig, socket_path: &Path) -> Vsock {
    Vsock {
        guest_cid: guest_cid_for(spec) as i64,
        uds_path: socket_path.display().to_string(),
        vsock_id: None,
    }
}

fn not_running(spec: &VmConfig) -> VmmError {
    VmmError::Backend(format!("machine {} is not running", spec.name))
}

fn fc_error(error: bento_fc::FirecrackerError) -> VmmError {
    VmmError::Backend(error.to_string())
}

fn vm_exit_from_status(status: ExitStatus) -> VmExit {
    if status.success() {
        return VmExit::Stopped;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(code) = status.code() {
            return VmExit::StoppedWithError(format!("firecracker exited with status code {code}"));
        }

        if let Some(signal) = status.signal() {
            return VmExit::StoppedWithError(format!("firecracker exited after signal {signal}"));
        }
    }

    VmExit::StoppedWithError("firecracker exited with an unknown status".to_string())
}

async fn shutdown_process(
    spec: &VmConfig,
    process: &bento_fc::FirecrackerProcess,
) -> Result<(), VmmError> {
    let shutdown_result = timeout(STOP_TIMEOUT, process.shutdown()).await;
    match shutdown_result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(fc_error(err)),
        Err(_) => {
            tracing::warn!(
                machine_id = spec.name.as_str(),
                timeout = ?STOP_TIMEOUT,
                "firecracker did not stop after SIGTERM, sending SIGKILL"
            );
            process.kill().await.map_err(fc_error)?;
            Ok(())
        }
    }
}

fn invalid_config<T>(spec: &VmConfig, reason: &str) -> Result<T, VmmError> {
    Err(VmmError::InvalidConfig {
        name: spec.name.clone(),
        reason: reason.to_string(),
    })
}
