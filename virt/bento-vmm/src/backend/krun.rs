use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

use crate::stream::{MachineSerialStream, VsockStream};
use crate::types::{Backend, DiskImage, NetworkMode, SharedDirectory, VmConfig, VmExit, VmmError};

const KRUN_BINARY_ENV: &str = "KRUN_BIN";
const KRUN_BINARY_NAME: &str = "krun";
const CONSOLE_LOG_NAME: &str = "krun.console.log";
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct KrunMachineBackend {
    config: VmConfig,
    krun_bin: PathBuf,
    runtime_dir: PathBuf,
    console_log_path: PathBuf,
    exit: Arc<Mutex<Option<VmExit>>>,
    runtime: AsyncMutex<Option<RunningKrun>>,
}

struct RunningKrun {
    child: Child,
}

impl std::fmt::Debug for KrunMachineBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KrunMachineBackend")
            .field("name", &self.config.name.as_str())
            .field("runtime_dir", &self.runtime_dir)
            .finish_non_exhaustive()
    }
}

pub(crate) fn validate(config: &VmConfig) -> Result<(), VmmError> {
    if config.cpus.is_none() {
        return invalid_config(config, "krun requires a CPU count");
    }
    if config.memory_mib.is_none() {
        return invalid_config(config, "krun requires a memory size");
    }
    if matches!(config.cpus, Some(0)) {
        return invalid_config(config, "krun requires at least one vCPU");
    }
    if matches!(config.memory_mib, Some(0)) {
        return invalid_config(config, "krun requires memory_mib to be greater than zero");
    }
    if config.kernel_path.is_none() {
        return invalid_config(config, "krun requires a kernel image path");
    }
    if config.initramfs_path.is_none() {
        return invalid_config(config, "krun requires an initramfs path");
    }
    if config.machine_identifier.is_some() {
        return invalid_config(
            config,
            "machine identifiers are not used by the krun backend",
        );
    }
    if config.rosetta {
        return invalid_config(config, "rosetta is not implemented for the krun backend");
    }
    if config.nested_virtualization {
        return invalid_config(
            config,
            "nested virtualization is not implemented for the krun backend yet",
        );
    }

    match config.network {
        NetworkMode::None => {}
        NetworkMode::VzNat => {
            return invalid_config(config, "krun networking is not implemented yet")
        }
        NetworkMode::Bridged => {
            return invalid_config(config, "bridged networking is not implemented for krun yet")
        }
        NetworkMode::Cni => {
            return invalid_config(config, "cni networking is not implemented for krun yet")
        }
    }

    Ok(())
}

fn prepare(config: &VmConfig) -> Result<(), VmmError> {
    validate(config)?;
    validate_support()?;
    if config.base_directory.as_os_str().is_empty() {
        return invalid_config(config, "base_directory must be set");
    }
    ensure_path_exists(
        config,
        config
            .kernel_path
            .as_ref()
            .expect("validated kernel missing"),
        "kernel image",
    )?;
    ensure_path_exists(
        config,
        config
            .initramfs_path
            .as_ref()
            .expect("validated initramfs missing"),
        "initramfs",
    )?;
    if let Some(root_disk) = config.root_disk.as_ref() {
        ensure_path_exists(config, &root_disk.path, "root disk")?;
    }
    for (index, disk) in config.data_disks.iter().enumerate() {
        ensure_path_exists(config, &disk.path, &format!("data disk #{index}"))?;
    }
    for mount in &config.mounts {
        ensure_path_exists(config, &mount.host_path, &format!("mount {}", mount.tag))?;
    }
    std::fs::create_dir_all(runtime_dir_for(config))?;
    Ok(())
}

impl KrunMachineBackend {
    pub(crate) fn new(config: VmConfig) -> Result<Self, VmmError> {
        validate(&config)?;
        let krun_bin = locate_krun_binary()?;
        let runtime_dir = runtime_dir_for(&config);
        let console_log_path = runtime_dir.join(CONSOLE_LOG_NAME);
        Ok(Self {
            config,
            krun_bin,
            runtime_dir,
            console_log_path,
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
        self.clear_exit_cache()?;
        if self.console_log_path.exists() {
            let _ = std::fs::remove_file(&self.console_log_path);
        }

        let mut command = Command::new(&self.krun_bin);
        command.arg("run");
        append_config_args(&mut command, &self.config, &self.console_log_path)?;
        let child = command.spawn()?;
        *runtime = Some(RunningKrun { child });
        Ok(())
    }

    pub(crate) async fn stop(&self) -> Result<(), VmmError> {
        let running = {
            let mut runtime = self.runtime.lock().await;
            runtime.take()
        };
        let Some(mut running) = running else {
            self.cache_exit(VmExit::Stopped)?;
            return Ok(());
        };
        if running.child.try_wait()?.is_none() {
            let _ = running.child.start_kill();
            let _ = timeout(STOP_TIMEOUT, running.child.wait()).await;
        }
        self.cache_exit(VmExit::Stopped)?;
        Ok(())
    }

    pub(crate) async fn connect_vsock(&self, _port: u32) -> Result<VsockStream, VmmError> {
        Err(VmmError::Unimplemented {
            kind: Backend::Krun,
            operation: "connect_vsock",
        })
    }

    pub(crate) async fn open_serial(&self) -> Result<MachineSerialStream, VmmError> {
        Err(VmmError::Unimplemented {
            kind: Backend::Krun,
            operation: "open_serial",
        })
    }

    pub(crate) async fn wait(&self) -> Result<VmExit, VmmError> {
        if let Some(exit) = self.cached_exit()? {
            return Ok(exit);
        }
        let mut runtime = self.runtime.lock().await;
        let Some(running) = runtime.as_mut() else {
            return Err(VmmError::Backend(
                "cannot wait for a virtual machine that has not been started".to_string(),
            ));
        };
        let status = running.child.wait().await?;
        let exit = vm_exit_from_status(status);
        *runtime = None;
        self.cache_exit(exit.clone())?;
        Ok(exit)
    }

    pub(crate) async fn try_wait(&self) -> Result<Option<VmExit>, VmmError> {
        if let Some(exit) = self.cached_exit()? {
            return Ok(Some(exit));
        }
        let mut runtime = self.runtime.lock().await;
        let Some(running) = runtime.as_mut() else {
            return Ok(None);
        };
        let Some(status) = running.child.try_wait()? else {
            return Ok(None);
        };
        let exit = vm_exit_from_status(status);
        *runtime = None;
        self.cache_exit(exit.clone())?;
        Ok(Some(exit))
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
}

fn append_config_args(
    command: &mut Command,
    config: &VmConfig,
    console_log_path: &Path,
) -> Result<(), VmmError> {
    command
        .arg("--cpus")
        .arg(config.cpus.expect("validated cpus missing").to_string())
        .arg("--memory-mib")
        .arg(
            config
                .memory_mib
                .expect("validated memory missing")
                .to_string(),
        )
        .arg("--kernel")
        .arg(
            config
                .kernel_path
                .as_ref()
                .expect("validated kernel missing"),
        )
        .arg("--initramfs")
        .arg(
            config
                .initramfs_path
                .as_ref()
                .expect("validated initramfs missing"),
        )
        .arg("--console-output")
        .arg(console_log_path);

    for arg in build_boot_args(config) {
        command.arg("--cmdline").arg(arg);
    }

    if let Some(root_disk) = config.root_disk.as_ref() {
        command.arg("--disk").arg(format_disk("root", root_disk));
    }
    for (index, disk) in config.data_disks.iter().enumerate() {
        command
            .arg("--disk")
            .arg(format_disk(&format!("disk{}", index + 1), disk));
    }
    for mount in &config.mounts {
        command.arg("--mount").arg(format_mount(mount));
    }
    Ok(())
}

fn build_boot_args(config: &VmConfig) -> Vec<String> {
    let mut args = vec!["console=hvc0".to_string(), "panic=1".to_string()];
    args.extend(config.kernel_cmdline.iter().cloned());
    args
}

fn format_disk(block_id: &str, disk: &DiskImage) -> String {
    format!(
        "{}:{}:{}",
        block_id,
        disk.path.display(),
        if disk.read_only { "ro" } else { "rw" }
    )
}

fn format_mount(mount: &SharedDirectory) -> String {
    format!(
        "{}:{}:{}",
        mount.tag,
        mount.host_path.display(),
        if mount.read_only { "ro" } else { "rw" }
    )
}

fn validate_support() -> Result<(), VmmError> {
    locate_krun_binary()?;
    Ok(())
}

fn locate_krun_binary() -> Result<PathBuf, VmmError> {
    if let Some(path) = env::var_os(KRUN_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(VmmError::UnsupportedBackend {
            kind: Backend::Krun,
            reason: format!(
                "{KRUN_BINARY_ENV} is set but does not point to a file: {}",
                path.display()
            ),
        });
    }

    if let Some(current_exe) = env::current_exe().ok() {
        if let Some(dir) = current_exe.parent() {
            let candidate = dir.join(KRUN_BINARY_NAME);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    let path = env::var_os("PATH").ok_or_else(|| VmmError::UnsupportedBackend {
        kind: Backend::Krun,
        reason: "PATH is not set, so the krun binary cannot be located".to_string(),
    })?;
    for entry in env::split_paths(&path) {
        let candidate = entry.join(KRUN_BINARY_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(VmmError::UnsupportedBackend {
        kind: Backend::Krun,
        reason: "krun binary was not found in PATH".to_string(),
    })
}

fn runtime_dir_for(config: &VmConfig) -> PathBuf {
    config.base_directory.clone()
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

fn vm_exit_from_status(status: ExitStatus) -> VmExit {
    if status.success() {
        return VmExit::Stopped;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(code) = status.code() {
            return VmExit::StoppedWithError(format!("krun exited with status code {code}"));
        }
        if let Some(signal) = status.signal() {
            return VmExit::StoppedWithError(format!("krun exited after signal {signal}"));
        }
    }
    VmExit::StoppedWithError("krun exited with an unknown status".to_string())
}

fn invalid_config<T>(config: &VmConfig, reason: &str) -> Result<T, VmmError> {
    Err(VmmError::InvalidConfig {
        name: config.name.clone(),
        reason: reason.to_string(),
    })
}
