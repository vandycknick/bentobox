use std::fs;
use std::io::{self, BufRead};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::images::store::ImageStore;
use crate::launch::prepare_instance_runtime;
use bento_core::InstanceFile;
use bento_core::{
    Architecture, Backend, Boot, Bootstrap, Disk, DiskKind, GuestOs, MachineId, Mount, Network,
    NetworkMode, Platform, Resources, Settings, Storage, VmSpec,
};
use bento_protocol::v1::InspectResponse;
use nix::{
    sys::signal::Signal,
    unistd::{pipe, Pid},
};

use crate::layout::CONFIG_FILE_NAME;
use crate::machine_ref::validate_machine_name;
use crate::monitor;
use crate::state::{metadata_from_path, MachineMetadata, StateStore};
use crate::{Layout, LibVmError, MachineRef};

const BYTES_PER_GB: u64 = 1_000_000_000;

#[derive(Debug, Clone)]
pub struct CreateMachineRequest {
    pub image_ref: String,
    pub name: String,
    pub cpus: Option<u8>,
    pub memory_mib: Option<u32>,
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    pub disk_size_gb: Option<u64>,
    pub nested_virtualization: bool,
    pub agent: bool,
    pub rosetta: bool,
    pub userdata: Option<PathBuf>,
    pub disks: Vec<PathBuf>,
    pub mounts: Vec<Mount>,
    pub network: Option<NetworkMode>,
}

#[derive(Debug, Clone)]
pub struct CreateRawMachineRequest {
    pub name: String,
    pub cpus: u8,
    pub memory_mib: u32,
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    pub rootfs: Option<PathBuf>,
    pub empty_rootfs_gb: Option<u64>,
    pub nested_virtualization: bool,
    pub agent: bool,
    pub rosetta: bool,
    pub disks: Vec<PathBuf>,
    pub mounts: Vec<Mount>,
    pub network: Option<NetworkMode>,
}

#[derive(Debug, Clone)]
pub struct MachineRecord {
    pub id: MachineId,
    pub spec: VmSpec,
    pub dir: PathBuf,
    pub status: MachineStatus,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineStatus {
    /// VM is running. `started_at` is the unix timestamp when it started
    /// (derived from the pidfile mtime).
    Running {
        started_at: i64,
    },
    Stopped,
}

impl MachineStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

pub struct LibVm {
    layout: Layout,
    state: StateStore,
}

pub struct PendingMachine {
    id: MachineId,
    spec: VmSpec,
    staged_dir: PathBuf,
    final_dir: PathBuf,
    committed: bool,
}

impl LibVm {
    pub fn new(layout: Layout) -> Result<Self, LibVmError> {
        let state = StateStore::open(&layout)?;
        Ok(Self { layout, state })
    }

    pub fn from_env() -> Result<Self, LibVmError> {
        Self::new(Layout::from_env()?)
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn create_from_image(
        &self,
        request: CreateMachineRequest,
    ) -> Result<MachineRecord, LibVmError> {
        if matches!(request.disk_size_gb, Some(0)) {
            return Err(LibVmError::InvalidCreateRequest {
                name: request.name,
                reason: "--disk-size must be greater than 0".to_string(),
            });
        }

        let mut image_store = ImageStore::open()?;
        let kernel_path = canonicalize_optional_existing_path(request.kernel.as_deref(), "kernel")?;
        let initramfs_path =
            canonicalize_optional_existing_path(request.initramfs.as_deref(), "initramfs")?;
        let userdata_path =
            canonicalize_optional_existing_path(request.userdata.as_deref(), "userdata")?;
        let disk_paths = canonicalize_existing_paths(&request.disks, "disk")?;

        let selected_image = match image_store.resolve(&request.image_ref)? {
            Some(image) => image,
            None => image_store.pull(&request.image_ref, None)?,
        };

        let bootstrap = (userdata_path.is_some() || request.rosetta).then(|| Bootstrap {
            cloud_init: userdata_path.clone(),
        });
        let guest_enabled = should_enable_guest(request.agent, bootstrap.as_ref());

        let resolved_kernel =
            kernel_path.or_else(|| image_store.image_kernel_path(&selected_image));
        let resolved_initramfs =
            initramfs_path.or_else(|| image_store.image_initramfs_path(&selected_image));
        let resolved_cpus = request.cpus.unwrap_or(selected_image.metadata.defaults.cpu);
        let resolved_memory = request
            .memory_mib
            .unwrap_or(selected_image.metadata.defaults.memory_mib);

        let spec = VmSpec {
            version: 2,
            name: request.name.clone(),
            platform: Platform {
                guest_os: guest_os_from_image(&selected_image.metadata.os)?,
                architecture: architecture_from_image(&selected_image.metadata.arch)?,
                backend: Backend::Auto,
            },
            resources: Resources {
                cpus: resolved_cpus,
                memory_mib: resolved_memory,
            },
            boot: Boot {
                kernel: resolved_kernel,
                initramfs: resolved_initramfs,
                kernel_cmdline: Vec::new(),
                bootstrap,
            },
            storage: Storage {
                disks: std::iter::once(Disk {
                    path: PathBuf::from(InstanceFile::RootDisk.as_str()),
                    kind: DiskKind::Root,
                    read_only: false,
                })
                .chain(disk_paths.into_iter().map(|path| Disk {
                    path,
                    kind: DiskKind::Data,
                    read_only: false,
                }))
                .collect(),
            },
            mounts: assign_mount_tags(request.mounts),
            vsock_endpoints: Vec::new(),
            network: Network {
                mode: request.network.unwrap_or_else(default_network_mode),
            },
            settings: Settings {
                nested_virtualization: request.nested_virtualization,
                rosetta: request.rosetta,
                guest_enabled,
            },
        };

        let pending = self.create_pending(spec)?;
        let rootfs_path = pending.dir().join(InstanceFile::RootDisk.as_str());
        image_store.clone_base_image(&selected_image, &rootfs_path)?;

        if let Some(size_bytes) = gigabytes_to_bytes_checked(request.disk_size_gb) {
            ImageStore::resize_raw_disk(&rootfs_path, size_bytes)?;
        }

        pending.commit(self)
    }

    pub fn create_raw(
        &self,
        request: CreateRawMachineRequest,
    ) -> Result<MachineRecord, LibVmError> {
        if request.rootfs.is_some() && request.empty_rootfs_gb.is_some() {
            return Err(LibVmError::InvalidCreateRequest {
                name: request.name,
                reason: "--rootfs and --empty-rootfs are mutually exclusive".to_string(),
            });
        }
        if matches!(request.empty_rootfs_gb, Some(0)) {
            return Err(LibVmError::InvalidCreateRequest {
                name: request.name,
                reason: "--empty-rootfs must be greater than 0".to_string(),
            });
        }

        let kernel_path = canonicalize_optional_existing_path(request.kernel.as_deref(), "kernel")?;
        let initramfs_path =
            canonicalize_optional_existing_path(request.initramfs.as_deref(), "initramfs")?;
        let rootfs_path = canonicalize_optional_existing_path(request.rootfs.as_deref(), "rootfs")?;
        let disk_paths = canonicalize_existing_paths(&request.disks, "disk")?;

        let bootstrap = request.rosetta.then_some(Bootstrap { cloud_init: None });
        let guest_enabled = should_enable_guest(request.agent, bootstrap.as_ref());

        let mut disks = Vec::new();
        if let Some(path) = rootfs_path.clone() {
            disks.push(Disk {
                path,
                kind: DiskKind::Root,
                read_only: false,
            });
        } else if request.empty_rootfs_gb.is_some() {
            disks.push(Disk {
                path: PathBuf::from(InstanceFile::RootDisk.as_str()),
                kind: DiskKind::Root,
                read_only: false,
            });
        }
        disks.extend(disk_paths.into_iter().map(|path| Disk {
            path,
            kind: DiskKind::Data,
            read_only: false,
        }));

        let spec = VmSpec {
            version: 2,
            name: request.name.clone(),
            platform: Platform {
                guest_os: GuestOs::Linux,
                architecture: host_architecture(),
                backend: Backend::Auto,
            },
            resources: Resources {
                cpus: request.cpus,
                memory_mib: request.memory_mib,
            },
            boot: Boot {
                kernel: kernel_path,
                initramfs: initramfs_path,
                kernel_cmdline: Vec::new(),
                bootstrap,
            },
            storage: Storage { disks },
            mounts: assign_mount_tags(request.mounts),
            vsock_endpoints: Vec::new(),
            network: Network {
                mode: request.network.unwrap_or_else(default_network_mode),
            },
            settings: Settings {
                nested_virtualization: request.nested_virtualization,
                rosetta: request.rosetta,
                guest_enabled,
            },
        };

        let pending = self.create_pending(spec)?;

        if let Some(size_gb) = request.empty_rootfs_gb {
            let rootfs = pending.dir().join(InstanceFile::RootDisk.as_str());
            fs::File::create(&rootfs)?.set_len(size_gb.saturating_mul(BYTES_PER_GB))?;
        }

        pending.commit(self)
    }

    pub fn create_pending(&self, spec: VmSpec) -> Result<PendingMachine, LibVmError> {
        validate_machine_name(&spec.name)?;

        if self
            .state
            .get_machine_by_name(spec.name.as_str())?
            .is_some()
        {
            return Err(LibVmError::MachineAlreadyExists {
                name: spec.name.clone(),
            });
        }

        let id = MachineId::new();
        let final_dir = self.layout.instance_dir(id);
        if final_dir.exists() {
            return Err(LibVmError::MachineIdAlreadyExists { id });
        }

        let staged_dir = create_staging_dir(&self.layout)?;
        let config = serde_yaml_ng::to_string(&spec).map_err(|source| {
            LibVmError::VmSpecSerializeFailed {
                name: spec.name.clone(),
                source,
            }
        })?;
        fs::write(staged_dir.join(CONFIG_FILE_NAME), config)?;

        Ok(PendingMachine {
            id,
            spec,
            staged_dir,
            final_dir,
            committed: false,
        })
    }

    pub fn inspect(&self, machine: &MachineRef) -> Result<MachineRecord, LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        self.machine_record(metadata)
    }

    pub fn list(&self) -> Result<Vec<MachineRecord>, LibVmError> {
        self.state
            .list_machines()?
            .into_iter()
            .map(|metadata| self.machine_record(metadata))
            .collect()
    }

    pub fn remove(&self, machine: &MachineRef) -> Result<(), LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        let pid_path = self.layout.monitor_pid_path(metadata.id);

        if pid_path.exists() {
            return Err(LibVmError::MachineAlreadyRunning {
                reference: metadata.name.clone(),
            });
        }

        match fs::remove_dir_all(&metadata.instance_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        self.state.remove_machine(&metadata)
    }

    pub async fn start(&self, machine: &MachineRef) -> Result<MachineRecord, LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        let pid_path = self.layout.monitor_pid_path(metadata.id);

        if pid_path.exists() {
            return Err(LibVmError::MachineAlreadyRunning {
                reference: metadata.name.clone(),
            });
        }

        prepare_instance_runtime(Path::new(&metadata.instance_dir)).map_err(|err| {
            LibVmError::InstancePreparationFailed {
                reference: metadata.name.clone(),
                message: err.to_string(),
            }
        })?;

        let startup_pipe = spawn_vmmon(Path::new(&metadata.instance_dir))?;
        wait_for_monitor_start(startup_pipe, &self.layout.monitor_trace_path(metadata.id)).await?;
        self.machine_record(metadata)
    }

    pub async fn wait_for_guest_running(
        &self,
        machine: &MachineRef,
        timeout: std::time::Duration,
    ) -> Result<(), LibVmError> {
        let (metadata, socket_path) = self.resolve_running_socket(machine)?;
        monitor::wait_for_guest_running(&socket_path, timeout)
            .await
            .map_err(|message| LibVmError::MonitorProtocol {
                reference: metadata.name,
                message,
            })
    }

    pub async fn stop(&self, machine: &MachineRef) -> Result<MachineRecord, LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        let pid_path = self.layout.monitor_pid_path(metadata.id);
        let pid = read_monitor_pid(&pid_path).map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => LibVmError::MachineNotRunning {
                reference: metadata.name.clone(),
            },
            _ => err.into(),
        })?;

        nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGINT)
            .map_err(|err| io::Error::other(err.to_string()))?;
        wait_for_monitor_stop(&pid_path, &metadata.name).await?;
        self.machine_record(metadata)
    }

    pub async fn get_status(&self, machine: &MachineRef) -> Result<InspectResponse, LibVmError> {
        let (metadata, socket_path) = self.resolve_running_socket(machine)?;
        monitor::get_vm_monitor_inspect(&socket_path)
            .await
            .map_err(|message| LibVmError::MonitorProtocol {
                reference: metadata.name,
                message,
            })
    }

    pub async fn open_serial_stream(
        &self,
        machine: &MachineRef,
    ) -> Result<tokio::net::UnixStream, LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        let socket_path = self.layout.monitor_socket_path(metadata.id);

        if !self.layout.monitor_pid_path(metadata.id).exists() {
            return Err(LibVmError::MachineNotRunning {
                reference: metadata.name.clone(),
            });
        }

        monitor::open_serial_stream(&socket_path)
            .await
            .map_err(|message| LibVmError::MonitorProtocol {
                reference: metadata.name,
                message,
            })
    }

    pub async fn open_shell_stream(
        &self,
        machine: &MachineRef,
        wait_for_guest_readiness: bool,
    ) -> Result<tokio::net::UnixStream, LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        let socket_path = self.layout.monitor_socket_path(metadata.id);

        if !self.layout.monitor_pid_path(metadata.id).exists() {
            return Err(LibVmError::MachineNotRunning {
                reference: metadata.name.clone(),
            });
        }

        if wait_for_guest_readiness {
            let machine_record = self.machine_record(metadata.clone())?;
            let should_wait = machine_record.spec.settings.guest_enabled;

            if should_wait {
                monitor::wait_for_shell_with_timeout(
                    &socket_path,
                    monitor::DEFAULT_GUEST_READINESS_TIMEOUT,
                    std::time::Duration::from_secs(1),
                )
                .await
                .map_err(|message| LibVmError::MonitorProtocol {
                    reference: metadata.name.clone(),
                    message,
                })?;
            }
        }

        monitor::open_shell_stream(&socket_path)
            .await
            .map_err(|message| LibVmError::MonitorProtocol {
                reference: metadata.name,
                message,
            })
    }

    fn resolve_metadata(&self, machine: &MachineRef) -> Result<MachineMetadata, LibVmError> {
        match machine {
            MachineRef::Id(id) => {
                self.state
                    .get_machine_by_id(*id)?
                    .ok_or_else(|| LibVmError::MachineNotFound {
                        reference: id.to_string(),
                    })
            }
            MachineRef::Name(name) => {
                self.state
                    .get_machine_by_name(name)?
                    .ok_or_else(|| LibVmError::MachineNotFound {
                        reference: name.clone(),
                    })
            }
            MachineRef::IdPrefix(prefix) => {
                let matches = self.state.get_machine_by_id_prefix(prefix)?;
                match matches.len() {
                    0 => Err(LibVmError::MachineNotFound {
                        reference: prefix.clone(),
                    }),
                    1 => Ok(matches.into_iter().next().expect("just checked len == 1")),
                    count => Err(LibVmError::AmbiguousIdPrefix {
                        prefix: prefix.clone(),
                        count,
                    }),
                }
            }
        }
    }

    fn machine_record(&self, metadata: MachineMetadata) -> Result<MachineRecord, LibVmError> {
        let dir = PathBuf::from(&metadata.instance_dir);
        let config_path = dir.join(CONFIG_FILE_NAME);
        let config = fs::read_to_string(&config_path)?;
        let spec =
            serde_yaml_ng::from_str(&config).map_err(|source| LibVmError::VmSpecLoadFailed {
                id: metadata.id,
                path: config_path,
                source,
            })?;

        let pid_path = self.layout.monitor_pid_path(metadata.id);
        let status = if pid_path.exists() {
            let started_at = std::fs::metadata(&pid_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            MachineStatus::Running { started_at }
        } else {
            MachineStatus::Stopped
        };

        Ok(MachineRecord {
            id: metadata.id,
            spec,
            dir,
            status,
            created_at: metadata.created_at,
        })
    }

    fn resolve_running_socket(
        &self,
        machine: &MachineRef,
    ) -> Result<(MachineMetadata, PathBuf), LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        if !self.layout.monitor_pid_path(metadata.id).exists() {
            return Err(LibVmError::MachineNotRunning {
                reference: metadata.name,
            });
        }

        let socket_path = self.layout.monitor_socket_path(metadata.id);
        Ok((metadata, socket_path))
    }
}

fn assign_mount_tags(mounts: Vec<Mount>) -> Vec<Mount> {
    mounts
        .into_iter()
        .enumerate()
        .map(|(index, mut mount)| {
            if mount.tag.trim().is_empty() {
                mount.tag = format!("mount{index}");
            }
            mount
        })
        .collect()
}

fn guest_os_from_image(os: &str) -> Result<GuestOs, LibVmError> {
    match os {
        "linux" => Ok(GuestOs::Linux),
        other => Err(LibVmError::UnsupportedImageGuestOs {
            os: other.to_string(),
        }),
    }
}

fn architecture_from_image(arch: &str) -> Result<Architecture, LibVmError> {
    match arch {
        "arm64" | "aarch64" => Ok(Architecture::Aarch64),
        "amd64" | "x86_64" => Ok(Architecture::X86_64),
        other => Err(LibVmError::UnsupportedImageArchitecture {
            arch: other.to_string(),
        }),
    }
}

fn host_architecture() -> Architecture {
    match std::env::consts::ARCH {
        "aarch64" => Architecture::Aarch64,
        _ => Architecture::X86_64,
    }
}

fn gigabytes_to_bytes(size_gb: u64) -> u64 {
    size_gb.saturating_mul(BYTES_PER_GB)
}

fn gigabytes_to_bytes_checked(size_gb: Option<u64>) -> Option<u64> {
    size_gb.map(gigabytes_to_bytes)
}

#[cfg(target_os = "macos")]
fn default_network_mode() -> NetworkMode {
    NetworkMode::User
}

#[cfg(not(target_os = "macos"))]
fn default_network_mode() -> NetworkMode {
    NetworkMode::None
}

fn canonicalize_optional_existing_path(
    path: Option<&Path>,
    kind: &str,
) -> Result<Option<PathBuf>, LibVmError> {
    let Some(path) = path else {
        return Ok(None);
    };

    Ok(Some(canonicalize_existing_path(path, kind)?))
}

fn canonicalize_existing_paths(paths: &[PathBuf], kind: &str) -> Result<Vec<PathBuf>, LibVmError> {
    paths
        .iter()
        .map(|path| canonicalize_existing_path(path, kind))
        .collect()
}

fn canonicalize_existing_path(path: &Path, kind: &str) -> Result<PathBuf, LibVmError> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    std::fs::canonicalize(&abs).map_err(|err| LibVmError::InvalidCreateRequest {
        name: kind.to_string(),
        reason: format!("{kind} path does not exist: {} ({err})", abs.display()),
    })
}

fn spawn_vmmon(instance_dir: &Path) -> Result<OwnedFd, LibVmError> {
    let (read_fd, write_fd) = pipe().map_err(|err| io::Error::other(err.to_string()))?;

    let mut command = Command::new(resolve_vmmon_executable()?);
    command.arg("--data-dir").arg(instance_dir);
    command
        .arg("--startup-fd")
        .arg(write_fd.as_raw_fd().to_string());

    // vmmon handles its own daemonization via double-fork internally,
    // so we just need to make sure the write end of the startup pipe
    // is inherited by the child. nix::pipe() sets FD_CLOEXEC by default,
    // so we need to clear it for the write fd.
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    let flags =
        fcntl(&write_fd, FcntlArg::F_GETFD).map_err(|err| io::Error::other(err.to_string()))?;
    let mut fd_flags = FdFlag::from_bits_retain(flags);
    fd_flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(&write_fd, FcntlArg::F_SETFD(fd_flags))
        .map_err(|err| io::Error::other(err.to_string()))?;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    drop(write_fd);

    Ok(read_fd)
}

fn resolve_vmmon_executable() -> Result<PathBuf, LibVmError> {
    let current_exe = std::env::current_exe()?;
    let expected_path = current_exe
        .parent()
        .map(|parent| parent.join("vmmon"))
        .unwrap_or_else(|| PathBuf::from("vmmon"));

    if expected_path.exists() {
        return Ok(expected_path);
    }

    if let Some(path) = std::env::var_os("PATH") {
        if std::env::split_paths(&path)
            .map(|path| path.join("vmmon"))
            .any(|candidate| candidate.exists())
        {
            return Ok(PathBuf::from("vmmon"));
        }
    }

    Err(LibVmError::VmMonExecutableNotFound { expected_path })
}

async fn wait_for_monitor_start(
    startup_pipe: OwnedFd,
    trace_path: &Path,
) -> Result<(), LibVmError> {
    let deadline_duration = std::time::Duration::from_secs(30);
    let trace_path = trace_path.to_path_buf();
    let result = tokio::time::timeout(
        deadline_duration,
        tokio::task::spawn_blocking(move || read_startup_pipe(startup_pipe)),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "vmmon startup pipe did not report readiness in {:?} (hint: see {})",
                deadline_duration,
                trace_path.display(),
            ),
        )
    })?
    .map_err(|err| io::Error::other(format!("join vmmon startup wait task: {err}")))??;

    match result {
        StartupResult::Started => Ok(()),
        StartupResult::Failed(message) => Err(io::Error::other(message).into()),
    }
}

fn read_startup_pipe(startup_pipe: OwnedFd) -> io::Result<StartupResult> {
    let mut input = String::new();
    let mut file = std::fs::File::from(startup_pipe);
    std::io::BufReader::new(&mut file).read_line(&mut input)?;

    if input == "started\n" {
        return Ok(StartupResult::Started);
    }

    if let Some(message) = input.strip_prefix("failed\t") {
        return Ok(StartupResult::Failed(message.trim_end().to_string()));
    }

    if input.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "vmmon exited before reporting startup result",
        ));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected vmmon startup message: {input:?}"),
    ))
}

enum StartupResult {
    Started,
    Failed(String),
}

async fn wait_for_monitor_stop(pid_path: &Path, machine_name: &str) -> Result<(), LibVmError> {
    let timeout = std::time::Duration::from_secs(45);
    let poll_interval = std::time::Duration::from_millis(200);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        match tokio::fs::metadata(pid_path).await {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "timed out after {:?} waiting for machine {:?} to stop",
                    timeout, machine_name
                ),
            )
            .into());
        }

        tokio::time::sleep(poll_interval).await;
    }
}

fn read_monitor_pid(pid_path: &Path) -> io::Result<i32> {
    let raw = fs::read_to_string(pid_path)?;
    let trimmed = raw.trim();
    trimmed.parse::<i32>().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse monitor pid from {}: {err}", pid_path.display()),
        )
    })
}

impl PendingMachine {
    pub fn id(&self) -> MachineId {
        self.id
    }

    pub fn dir(&self) -> &Path {
        &self.staged_dir
    }

    pub fn spec(&self) -> &VmSpec {
        &self.spec
    }

    pub fn commit(mut self, libvm: &LibVm) -> Result<MachineRecord, LibVmError> {
        if self.final_dir.exists() {
            return Err(LibVmError::MachineIdAlreadyExists { id: self.id });
        }

        if let Some(parent) = self.final_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&self.staged_dir, &self.final_dir)?;

        let metadata = metadata_from_path(self.id, self.spec.name.clone(), &self.final_dir);
        if let Err(err) = libvm.state.insert_machine(&metadata) {
            let _ = fs::remove_dir_all(&self.final_dir);
            return Err(err);
        }

        self.committed = true;
        libvm.machine_record(metadata)
    }
}

impl Drop for PendingMachine {
    fn drop(&mut self) {
        if self.committed {
            return;
        }

        match fs::remove_dir_all(&self.staged_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

fn create_staging_dir(layout: &Layout) -> Result<PathBuf, LibVmError> {
    let staging_root = layout.staging_dir();
    fs::create_dir_all(&staging_root)?;

    for attempt in 0..256u32 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| LibVmError::InvalidMachineName {
                name: "staging".to_string(),
                reason: format!("system clock error while creating staging dir: {err}"),
            })?
            .as_nanos();
        let candidate = staging_root.join(format!("{}-{timestamp}-{attempt}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }

    Err(LibVmError::InvalidMachineName {
        name: "staging".to_string(),
        reason: "failed to allocate unique staging directory".to_string(),
    })
}

fn should_enable_guest(agent: bool, bootstrap: Option<&Bootstrap>) -> bool {
    agent || bootstrap.is_some()
}

#[cfg(test)]
mod tests {
    use super::{should_enable_guest, LibVm, MachineStatus};
    use crate::{Layout, LibVmError, MachineRef};
    use bento_core::{
        Architecture, Backend, Boot, GuestOs, Network, NetworkMode, Platform, Resources, Settings,
        Storage, VmSpec,
    };

    fn sample_vm_spec(name: &str) -> VmSpec {
        VmSpec {
            version: 2,
            name: name.to_string(),
            platform: Platform {
                guest_os: GuestOs::Linux,
                architecture: Architecture::Aarch64,
                backend: Backend::Auto,
            },
            resources: Resources {
                cpus: 4,
                memory_mib: 4096,
            },
            boot: Boot {
                kernel: None,
                initramfs: None,
                kernel_cmdline: Vec::new(),
                bootstrap: None,
            },
            storage: Storage { disks: Vec::new() },
            mounts: Vec::new(),
            vsock_endpoints: Vec::new(),
            network: Network {
                mode: NetworkMode::User,
            },
            settings: Settings {
                nested_virtualization: false,
                rosetta: false,
                guest_enabled: true,
            },
        }
    }

    #[test]
    fn create_pending_and_commit_write_vm_spec_and_state() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let libvm = LibVm::new(Layout::new(temp.path().join("bento"))).expect("create libvm");

        let pending = libvm
            .create_pending(sample_vm_spec("devbox"))
            .expect("create pending machine");
        let machine_id = pending.id();

        assert!(pending.dir().starts_with(libvm.layout().staging_dir()));

        let machine = pending.commit(&libvm).expect("commit machine");

        assert_eq!(machine.id, machine_id);
        assert_eq!(machine.spec.name, "devbox");
        assert_eq!(machine.status, MachineStatus::Stopped);
        assert_eq!(machine.dir, libvm.layout().instance_dir(machine_id));
        assert!(libvm.layout().instance_config_path(machine_id).exists());
    }

    #[test]
    fn inspect_and_list_use_name_and_id_lookup() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let libvm = LibVm::new(Layout::new(temp.path().join("bento"))).expect("create libvm");

        let machine = libvm
            .create_pending(sample_vm_spec("devbox"))
            .expect("create pending machine")
            .commit(&libvm)
            .expect("commit machine");

        let by_name = libvm
            .inspect(&MachineRef::parse("devbox").expect("parse machine ref"))
            .expect("inspect by name");
        let by_id = libvm
            .inspect(&MachineRef::parse(machine.id.to_string()).expect("parse machine ref"))
            .expect("inspect by id");
        let listed = libvm.list().expect("list machines");

        assert_eq!(by_name.id, machine.id);
        assert_eq!(by_id.id, machine.id);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].spec.name, "devbox");
    }

    #[test]
    fn remove_deletes_machine_from_state_and_disk() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let libvm = LibVm::new(Layout::new(temp.path().join("bento"))).expect("create libvm");

        let machine = libvm
            .create_pending(sample_vm_spec("devbox"))
            .expect("create pending machine")
            .commit(&libvm)
            .expect("commit machine");

        libvm
            .remove(&MachineRef::parse(machine.id.to_string()).expect("parse machine ref"))
            .expect("remove machine");

        assert!(!machine.dir.exists());
        assert!(libvm.list().expect("list machines").is_empty());
    }

    #[test]
    fn remove_refuses_running_machine_when_pid_file_exists() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let libvm = LibVm::new(Layout::new(temp.path().join("bento"))).expect("create libvm");

        let machine = libvm
            .create_pending(sample_vm_spec("devbox"))
            .expect("create pending machine")
            .commit(&libvm)
            .expect("commit machine");

        let pid_path = libvm.layout().monitor_pid_path(machine.id);
        std::fs::write(&pid_path, b"12345\n").expect("write pid file");

        let err = libvm
            .remove(&MachineRef::parse(machine.id.to_string()).expect("parse machine ref"))
            .expect_err("removing running machine should fail");

        assert!(matches!(
            err,
            LibVmError::MachineAlreadyRunning { ref reference } if reference == "devbox"
        ));
        assert!(machine.dir.exists());
        assert_eq!(libvm.list().expect("list machines").len(), 1);
    }

    #[test]
    fn should_enable_guest_when_agent_is_requested() {
        assert!(should_enable_guest(true, None));
    }

    #[test]
    fn should_enable_guest_when_bootstrap_is_present() {
        let bootstrap = bento_core::Bootstrap { cloud_init: None };
        assert!(should_enable_guest(false, Some(&bootstrap)));
    }

    #[test]
    fn should_not_enable_guest_without_agent_or_bootstrap() {
        assert!(!should_enable_guest(false, None));
    }
}
