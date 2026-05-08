use std::env;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bento_ch::types::{
    ConsoleConfig, ConsoleMode, CpusConfig, DiskConfig, FsConfig, MemoryConfig, PayloadConfig,
    VsockConfig,
};
use bento_ch::{CloudHypervisorProcess, CloudHypervisorProcessBuilder};
use tokio::net::UnixStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{sleep, timeout};

use crate::stream::{MachineSerialStream, VsockStream};
use crate::types::{Backend, DiskImage, NetworkMode, SharedDirectory, VmConfig, VmExit, VmmError};

const CLOUD_HYPERVISOR_BINARY_ENV: &str = "CLOUD_HYPERVISOR_BIN";
const CLOUD_HYPERVISOR_BINARY_NAME: &str = "cloud-hypervisor";
const VIRTIOFSD_BINARY_ENV: &str = "VIRTIOFSD_BIN";
const VIRTIOFSD_BINARY_NAME: &str = "virtiofsd";
const API_SOCKET_NAME: &str = "cloud-hypervisor.sock";
const SERIAL_SOCKET_NAME: &str = "cloud-hypervisor.serial.sock";
const VSOCK_SOCKET_NAME: &str = "cloud-hypervisor.vsock";
const GUEST_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const VIRTIOFSD_SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const VIRTIOFSD_SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) struct CloudHypervisorMachineBackend {
    config: VmConfig,
    cloud_hypervisor_bin: PathBuf,
    virtiofsd_bin: Option<PathBuf>,
    runtime_dir: PathBuf,
    api_socket_path: PathBuf,
    serial_socket_path: PathBuf,
    vsock_socket_path: PathBuf,
    exit: Arc<Mutex<Option<VmExit>>>,
    runtime: AsyncMutex<Option<RunningCloudHypervisor>>,
}

struct RunningCloudHypervisor {
    process: Arc<CloudHypervisorProcess>,
    vm: bento_ch::VirtualMachine,
    fs_daemons: Vec<VirtioFsDaemon>,
}

struct VirtioFsDaemon {
    socket_path: PathBuf,
    process: Child,
}

impl std::fmt::Debug for CloudHypervisorMachineBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudHypervisorMachineBackend")
            .field("name", &self.config.name.as_str())
            .field("runtime_dir", &self.runtime_dir)
            .finish_non_exhaustive()
    }
}

pub(crate) fn validate(config: &VmConfig) -> Result<(), VmmError> {
    let machine = config;
    if machine.cpus.is_none() {
        return invalid_config(config, "cloud-hypervisor requires a CPU count");
    }
    if machine.memory_mib.is_none() {
        return invalid_config(config, "cloud-hypervisor requires a memory size");
    }
    if machine.kernel_path.is_none() {
        return invalid_config(config, "cloud-hypervisor requires a kernel image path");
    }
    if machine.initramfs_path.is_none() {
        return invalid_config(config, "cloud-hypervisor requires an initramfs path");
    }
    if matches!(machine.cpus, Some(0)) {
        return invalid_config(config, "cloud-hypervisor requires at least one vCPU");
    }
    if matches!(machine.memory_mib, Some(0)) {
        return invalid_config(
            config,
            "cloud-hypervisor requires memory_mib to be greater than zero",
        );
    }
    if machine.machine_identifier.is_some() {
        return invalid_config(
            config,
            "machine identifiers are not used by the cloud-hypervisor backend",
        );
    }
    if machine.nested_virtualization {
        return invalid_config(
            config,
            "nested virtualization is not implemented for the cloud-hypervisor backend yet",
        );
    }
    if machine.rosetta {
        return invalid_config(
            config,
            "rosetta is not implemented for the cloud-hypervisor backend",
        );
    }

    match machine.network {
        NetworkMode::None => {}
        NetworkMode::User => {
            return invalid_config(
                config,
                "user networking is not implemented for the cloud-hypervisor backend yet",
            );
        }
        NetworkMode::VzNat => {
            return invalid_config(
                config,
                "vznat networking is only supported by the VZ backend",
            );
        }
    }

    Ok(())
}

fn prepare(config: &VmConfig) -> Result<(), VmmError> {
    validate(config)?;
    validate_support(config)?;

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
    for mount in &config.mounts {
        ensure_path_exists(config, &mount.host_path, &format!("mount {}", mount.tag))?;
    }

    std::fs::create_dir_all(runtime_dir_for(config))?;
    Ok(())
}

impl CloudHypervisorMachineBackend {
    pub(crate) fn new(config: VmConfig) -> Result<Self, VmmError> {
        validate(&config)?;
        let cloud_hypervisor_bin = locate_cloud_hypervisor_binary()?;
        let virtiofsd_bin = if config.mounts.is_empty() {
            None
        } else {
            Some(locate_virtiofsd_binary()?)
        };
        let runtime_dir = runtime_dir_for(&config);
        let api_socket_path = runtime_dir.join(API_SOCKET_NAME);
        let serial_socket_path = runtime_dir.join(SERIAL_SOCKET_NAME);
        let vsock_socket_path = runtime_dir.join(VSOCK_SOCKET_NAME);

        Ok(Self {
            config,
            cloud_hypervisor_bin,
            virtiofsd_bin,
            runtime_dir,
            api_socket_path,
            serial_socket_path,
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

        let mut fs_daemons = match self.launch_virtiofs_daemons().await {
            Ok(fs_daemons) => fs_daemons,
            Err(err) => {
                self.cleanup_runtime_files();
                return Err(err);
            }
        };

        let process = Arc::new(
            CloudHypervisorProcessBuilder::new(&self.cloud_hypervisor_bin, &self.api_socket_path)
                .spawn()
                .await
                .map_err(ch_error)?,
        );

        let vm_result = process
            .builder()
            .cpus(build_cpus(&self.config)?)
            .memory(build_memory(&self.config))
            .payload(build_payload(&self.config)?)
            .serial(build_serial_console(&self.serial_socket_path))
            .console(disabled_console())
            .set_vsock(build_vsock(&self.config, &self.vsock_socket_path))
            .add_disk_if_some(build_root_disk(&self.config)?)
            .add_disks(build_data_disks(&self.config)?)
            .add_fs_mounts(build_fs_mounts(&self.config, &fs_daemons))
            .start()
            .await;

        let vm = match vm_result {
            Ok(vm) => vm,
            Err(err) => {
                let _ = timeout(STOP_TIMEOUT, process.shutdown()).await;
                cleanup_fs_daemons(&mut fs_daemons);
                self.cleanup_runtime_files();
                return Err(ch_error(err));
            }
        };

        *runtime = Some(RunningCloudHypervisor {
            process,
            vm,
            fs_daemons,
        });
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

        let mut shutdown_error = None;

        if running.process.try_wait().map_err(ch_error)?.is_none() {
            match running.vm.shutdown().await {
                Ok(()) => match timeout(GUEST_SHUTDOWN_TIMEOUT, running.process.wait()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(err)) => shutdown_error = Some(ch_error(err)),
                    Err(_) => {
                        tracing::warn!(
                            machine_id = self.config.name.as_str(),
                            timeout = ?GUEST_SHUTDOWN_TIMEOUT,
                            "guest did not shut down after vm.shutdown, falling back to VMM shutdown"
                        );
                        if let Err(err) = shutdown_process(&self.config, &running.process).await {
                            shutdown_error = Some(err);
                        }
                    }
                },
                Err(err) => {
                    tracing::warn!(
                        machine_id = self.config.name.as_str(),
                        error = %err,
                        "failed to request cloud-hypervisor guest shutdown, falling back to VMM shutdown"
                    );
                    if let Err(stop_err) = shutdown_process(&self.config, &running.process).await {
                        shutdown_error = Some(stop_err);
                    }
                }
            }
        }

        cleanup_fs_daemons(&mut running.fs_daemons);

        if let Some(err) = shutdown_error {
            return Err(err);
        }

        self.cache_exit(VmExit::Stopped)?;
        self.cleanup_runtime_files();
        Ok(())
    }

    pub(crate) async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmmError> {
        let vsock = {
            let runtime = self.runtime.lock().await;
            let running = runtime.as_ref().ok_or_else(|| not_running(&self.config))?;
            running.vm.vsock().map_err(ch_error)?
        };

        let stream = vsock.connect(port).await.map_err(ch_error)?;
        Ok(VsockStream::from_cloud_hypervisor(stream))
    }

    pub(crate) async fn open_serial(&self) -> Result<MachineSerialStream, VmmError> {
        let socket_path = {
            let runtime = self.runtime.lock().await;
            let _running = runtime.as_ref().ok_or_else(|| not_running(&self.config))?;
            self.serial_socket_path.clone()
        };

        let stream = UnixStream::connect(&socket_path).await?;
        Ok(MachineSerialStream::from_unix_stream(stream))
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

        let status = process.wait().await.map_err(ch_error)?;
        let exit = vm_exit_from_status(status);

        let mut runtime = self.runtime.lock().await;
        if let Some(mut running) = runtime.take() {
            cleanup_fs_daemons(&mut running.fs_daemons);
        }
        self.cache_exit(exit.clone())?;
        self.cleanup_runtime_files();
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

        let Some(status) = running.process.try_wait().map_err(ch_error)? else {
            return Ok(None);
        };

        let exit = vm_exit_from_status(status);
        cleanup_fs_daemons(&mut running.fs_daemons);
        *runtime = None;
        self.cache_exit(exit.clone())?;
        self.cleanup_runtime_files();
        Ok(Some(exit))
    }

    async fn launch_virtiofs_daemons(&self) -> Result<Vec<VirtioFsDaemon>, VmmError> {
        let Some(virtiofsd_bin) = self.virtiofsd_bin.as_ref() else {
            return Ok(Vec::new());
        };

        let mut daemons = Vec::new();
        for mount in &self.config.mounts {
            match spawn_virtiofsd(virtiofsd_bin, &self.runtime_dir, mount).await {
                Ok(daemon) => daemons.push(daemon),
                Err(err) => {
                    cleanup_fs_daemons(&mut daemons);
                    return Err(err);
                }
            }
        }

        Ok(daemons)
    }

    fn ensure_runtime_dir(&self) -> Result<(), VmmError> {
        std::fs::create_dir_all(&self.runtime_dir)?;
        for path in [
            &self.api_socket_path,
            &self.serial_socket_path,
            &self.vsock_socket_path,
        ] {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
        for mount in &self.config.mounts {
            let socket_path = virtiofs_socket_path(&self.runtime_dir, mount);
            if socket_path.exists() {
                let _ = std::fs::remove_file(socket_path);
            }
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
        for path in [
            &self.api_socket_path,
            &self.serial_socket_path,
            &self.vsock_socket_path,
        ] {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
        for mount in &self.config.mounts {
            let socket_path = virtiofs_socket_path(&self.runtime_dir, mount);
            if socket_path.exists() {
                let _ = std::fs::remove_file(socket_path);
            }
        }
    }
}

trait VirtualMachineBuilderExt {
    fn add_disk_if_some(self, disk: Option<DiskConfig>) -> Self;
    fn add_disks(self, disks: Vec<DiskConfig>) -> Self;
    fn add_fs_mounts(self, mounts: Vec<FsConfig>) -> Self;
}

impl VirtualMachineBuilderExt for bento_ch::VirtualMachineBuilder {
    fn add_disk_if_some(self, disk: Option<DiskConfig>) -> Self {
        match disk {
            Some(disk) => self.add_disk(disk),
            None => self,
        }
    }

    fn add_disks(self, disks: Vec<DiskConfig>) -> Self {
        disks
            .into_iter()
            .fold(self, |builder, disk| builder.add_disk(disk))
    }

    fn add_fs_mounts(self, mounts: Vec<FsConfig>) -> Self {
        mounts
            .into_iter()
            .fold(self, |builder, mount| builder.add_fs(mount))
    }
}

fn validate_support(config: &VmConfig) -> Result<(), VmmError> {
    locate_cloud_hypervisor_binary()?;
    if !Path::new("/dev/kvm").exists() {
        return Err(VmmError::UnsupportedBackend {
            kind: Backend::CloudHypervisor,
            reason: "cloud-hypervisor requires /dev/kvm on Linux hosts".to_string(),
        });
    }
    if !config.mounts.is_empty() {
        locate_virtiofsd_binary()?;
    }
    Ok(())
}

fn locate_cloud_hypervisor_binary() -> Result<PathBuf, VmmError> {
    locate_binary(
        CLOUD_HYPERVISOR_BINARY_ENV,
        CLOUD_HYPERVISOR_BINARY_NAME,
        Backend::CloudHypervisor,
        "cloud-hypervisor",
    )
}

fn locate_virtiofsd_binary() -> Result<PathBuf, VmmError> {
    locate_binary(
        VIRTIOFSD_BINARY_ENV,
        VIRTIOFSD_BINARY_NAME,
        Backend::CloudHypervisor,
        "virtiofsd",
    )
}

fn locate_binary(
    env_key: &str,
    binary_name: &str,
    backend: Backend,
    label: &str,
) -> Result<PathBuf, VmmError> {
    if let Some(path) = env::var_os(env_key) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }

        return Err(VmmError::UnsupportedBackend {
            kind: backend,
            reason: format!(
                "{env_key} is set but does not point to a file: {}",
                path.display()
            ),
        });
    }

    let path = env::var_os("PATH").ok_or_else(|| VmmError::UnsupportedBackend {
        kind: backend,
        reason: format!("PATH is not set, so the {label} binary cannot be located"),
    })?;

    for entry in env::split_paths(&path) {
        let candidate = entry.join(binary_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(VmmError::UnsupportedBackend {
        kind: backend,
        reason: format!("{label} binary was not found in PATH"),
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

fn build_payload(spec: &VmConfig) -> Result<PayloadConfig, VmmError> {
    Ok(PayloadConfig {
        cmdline: Some(build_boot_args(spec)),
        firmware: None,
        host_data: None,
        igvm: None,
        initramfs: Some(
            spec.initramfs_path
                .as_ref()
                .expect("validated initramfs path missing")
                .display()
                .to_string(),
        ),
        kernel: Some(
            spec.kernel_path
                .as_ref()
                .expect("validated kernel path missing")
                .display()
                .to_string(),
        ),
    })
}

fn build_boot_args(config: &VmConfig) -> String {
    let mut args = vec![
        serial_console_kernel_arg().to_string(),
        "reboot=k".to_string(),
        "panic=1".to_string(),
    ];
    if config.root_disk.is_some() {
        args.push("root=/dev/vda".to_string());
        args.push("rw".to_string());
    }
    args.extend(config.kernel_cmdline.iter().cloned());
    args.join(" ")
}

#[cfg(target_arch = "aarch64")]
fn serial_console_kernel_arg() -> &'static str {
    "console=ttyAMA0"
}

#[cfg(not(target_arch = "aarch64"))]
fn serial_console_kernel_arg() -> &'static str {
    "console=ttyS0"
}

fn build_cpus(spec: &VmConfig) -> Result<CpusConfig, VmmError> {
    let count = u64::try_from(spec.cpus.expect("validated cpus missing")).map_err(|_| {
        VmmError::InvalidConfig {
            name: spec.name.clone(),
            reason: "vCPU count does not fit in u64".to_string(),
        }
    })?;
    let count = NonZeroU64::new(count).ok_or_else(|| VmmError::InvalidConfig {
        name: spec.name.clone(),
        reason: "cloud-hypervisor requires at least one vCPU".to_string(),
    })?;

    Ok(CpusConfig {
        affinity: Vec::new(),
        boot_vcpus: count,
        core_scheduling: None,
        features: None,
        kvm_hyperv: false,
        max_phys_bits: None,
        max_vcpus: count,
        nested: false,
        topology: None,
    })
}

fn build_memory(spec: &VmConfig) -> MemoryConfig {
    MemoryConfig {
        hotplug_method: "Acpi".to_string(),
        hotplug_size: None,
        hotplugged_size: None,
        hugepage_size: None,
        hugepages: false,
        mergeable: false,
        prefault: false,
        shared: !spec.mounts.is_empty(),
        size: i64::try_from(spec.memory_mib.expect("validated memory missing")).unwrap_or(0)
            * 1024
            * 1024,
        thp: true,
        zones: Vec::new(),
    }
}

fn build_root_disk(spec: &VmConfig) -> Result<Option<DiskConfig>, VmmError> {
    spec.root_disk.as_ref().map(build_disk).transpose()
}

fn build_data_disks(spec: &VmConfig) -> Result<Vec<DiskConfig>, VmmError> {
    spec.data_disks.iter().map(build_disk).collect()
}

fn build_disk(disk: &DiskImage) -> Result<DiskConfig, VmmError> {
    Ok(DiskConfig {
        path: Some(disk.path.display().to_string()),
        readonly: disk.read_only,
        ..Default::default()
    })
}

fn build_vsock(spec: &VmConfig, socket_path: &Path) -> VsockConfig {
    VsockConfig {
        cid: guest_cid_for(spec) as i64,
        id: None,
        iommu: false,
        pci_segment: None,
        socket: socket_path.display().to_string(),
    }
}

fn build_serial_console(socket_path: &Path) -> ConsoleConfig {
    ConsoleConfig {
        file: None,
        iommu: false,
        mode: ConsoleMode::Socket,
        socket: Some(socket_path.display().to_string()),
    }
}

fn disabled_console() -> ConsoleConfig {
    ConsoleConfig {
        file: None,
        iommu: false,
        mode: ConsoleMode::Off,
        socket: None,
    }
}

fn build_fs_mounts(spec: &VmConfig, daemons: &[VirtioFsDaemon]) -> Vec<FsConfig> {
    spec.mounts
        .iter()
        .zip(daemons.iter())
        .map(|(mount, daemon)| FsConfig {
            id: None,
            num_queues: 1,
            pci_segment: None,
            queue_size: 1024,
            socket: daemon.socket_path.display().to_string(),
            tag: mount.tag.clone(),
        })
        .collect()
}

fn guest_cid_for(spec: &VmConfig) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.name.as_str().hash(&mut hasher);
    3u32 + (hasher.finish() as u32 % 0x3fff_fffc)
}

fn virtiofs_socket_path(runtime_dir: &Path, mount: &SharedDirectory) -> PathBuf {
    runtime_dir.join(format!("virtiofsd-{}.sock", mount.tag))
}

async fn spawn_virtiofsd(
    virtiofsd_bin: &Path,
    runtime_dir: &Path,
    mount: &SharedDirectory,
) -> Result<VirtioFsDaemon, VmmError> {
    let socket_path = virtiofs_socket_path(runtime_dir, mount);
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let mut process = std::process::Command::new(virtiofsd_bin)
        .arg(format!("--socket-path={}", socket_path.display()))
        .arg(format!("--shared-dir={}", mount.host_path.display()))
        .arg("--cache=never")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(VmmError::Io)?;

    wait_for_virtiofs_socket(&socket_path, &mut process).await?;

    Ok(VirtioFsDaemon {
        socket_path,
        process,
    })
}

async fn wait_for_virtiofs_socket(path: &Path, process: &mut Child) -> Result<(), VmmError> {
    let path = path.to_path_buf();
    timeout(VIRTIOFSD_SOCKET_TIMEOUT, async {
        loop {
            if path.exists() {
                return Ok(());
            }

            if process.try_wait()?.is_some() {
                return Err(VmmError::Backend(
                    "virtiofsd exited before its socket became ready".to_string(),
                ));
            }

            sleep(VIRTIOFSD_SOCKET_POLL_INTERVAL).await;
        }
    })
    .await
    .map_err(|_| {
        VmmError::Backend(format!(
            "timed out waiting for virtiofsd socket at {}",
            path.display()
        ))
    })?
}

fn cleanup_fs_daemons(daemons: &mut Vec<VirtioFsDaemon>) {
    for daemon in daemons.iter_mut() {
        let _ = daemon.process.kill();
        let _ = daemon.process.wait();
        if daemon.socket_path.exists() {
            let _ = std::fs::remove_file(&daemon.socket_path);
        }
    }
    daemons.clear();
}

fn not_running(spec: &VmConfig) -> VmmError {
    VmmError::Backend(format!("machine {} is not running", spec.name))
}

fn ch_error(error: bento_ch::CloudHypervisorError) -> VmmError {
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
            return VmExit::StoppedWithError(format!(
                "cloud-hypervisor exited with status code {code}"
            ));
        }

        if let Some(signal) = status.signal() {
            return VmExit::StoppedWithError(format!(
                "cloud-hypervisor exited after signal {signal}"
            ));
        }
    }

    VmExit::StoppedWithError("cloud-hypervisor exited with an unknown status".to_string())
}

async fn shutdown_process(
    spec: &VmConfig,
    process: &CloudHypervisorProcess,
) -> Result<(), VmmError> {
    let shutdown_result = timeout(STOP_TIMEOUT, process.shutdown()).await;
    match shutdown_result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(ch_error(err)),
        Err(_) => {
            tracing::warn!(
                machine_id = spec.name.as_str(),
                timeout = ?STOP_TIMEOUT,
                "cloud-hypervisor did not stop after VMM shutdown, sending SIGKILL"
            );
            process.kill().await.map_err(ch_error)?;
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
