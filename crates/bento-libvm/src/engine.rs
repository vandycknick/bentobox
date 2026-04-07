use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use bento_core::{MachineId, VmSpec};
use nix::{
    fcntl::{fcntl, FcntlArg, FdFlag},
    sys::signal::Signal,
    unistd::{pipe, Pid},
};

use crate::layout::CONFIG_FILE_NAME;
use crate::machine_ref::validate_machine_name;
use crate::state::{metadata_from_path, MachineMetadata, StateStore};
use crate::{Layout, LibVmError, MachineRef};

#[derive(Debug, Clone)]
pub struct MachineRecord {
    pub id: MachineId,
    pub spec: VmSpec,
    pub dir: PathBuf,
    pub status: MachineStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineStatus {
    Running,
    Stopped,
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
        match fs::remove_dir_all(&metadata.instance_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        self.state.remove_machine(&metadata)
    }

    pub async fn start(
        &self,
        machine: &MachineRef,
        profiles: &[String],
    ) -> Result<MachineRecord, LibVmError> {
        let metadata = self.resolve_metadata(machine)?;
        let pid_path = self.layout.monitor_pid_path(metadata.id);

        if pid_path.exists() {
            return Err(LibVmError::MachineAlreadyRunning {
                reference: metadata.name.clone(),
            });
        }

        let startup_pipe = spawn_vmmon(Path::new(&metadata.instance_dir), profiles)?;
        wait_for_monitor_start(startup_pipe, &self.layout.monitor_trace_path(metadata.id)).await?;
        self.machine_record(metadata)
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

    fn resolve_metadata(&self, machine: &MachineRef) -> Result<MachineMetadata, LibVmError> {
        let metadata = match machine {
            MachineRef::Id(id) => self.state.get_machine_by_id(*id)?,
            MachineRef::Name(name) => self.state.get_machine_by_name(name)?,
        };

        metadata.ok_or_else(|| LibVmError::MachineNotFound {
            reference: match machine {
                MachineRef::Id(id) => id.to_string(),
                MachineRef::Name(name) => name.clone(),
            },
        })
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

        Ok(MachineRecord {
            id: metadata.id,
            spec,
            dir,
            status: if self.layout.monitor_pid_path(metadata.id).exists() {
                MachineStatus::Running
            } else {
                MachineStatus::Stopped
            },
        })
    }
}

fn spawn_vmmon(instance_dir: &Path, profiles: &[String]) -> Result<OwnedFd, LibVmError> {
    let (read_fd, write_fd) = pipe().map_err(|err| io::Error::other(err.to_string()))?;
    let flags =
        fcntl(&write_fd, FcntlArg::F_GETFD).map_err(|err| io::Error::other(err.to_string()))?;
    let mut fd_flags = FdFlag::from_bits_retain(flags);
    fd_flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(&write_fd, FcntlArg::F_SETFD(fd_flags))
        .map_err(|err| io::Error::other(err.to_string()))?;

    let mut command = Command::new(resolve_vmmon_executable());
    command.arg("--data-dir").arg(instance_dir);
    command
        .arg("--startup-fd")
        .arg(write_fd.as_raw_fd().to_string());
    for profile in profiles {
        command.arg("--profile").arg(profile);
    }
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
        });
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    drop(write_fd);

    Ok(read_fd)
}

fn resolve_vmmon_executable() -> PathBuf {
    let fallback = PathBuf::from("vmmon");

    let Ok(current_exe) = std::env::current_exe() else {
        return fallback;
    };
    let Some(parent) = current_exe.parent() else {
        return fallback;
    };

    let sibling = parent.join("vmmon");
    if sibling.exists() {
        sibling
    } else {
        fallback
    }
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
    file.read_to_string(&mut input)?;

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

#[cfg(test)]
mod tests {
    use super::{LibVm, MachineStatus};
    use crate::{Layout, MachineRef};
    use bento_core::{
        Architecture, Backend, Boot, Capabilities, Guest, GuestOs, Host, Network, NetworkMode,
        Platform, Resources, Storage, VmSpec,
    };

    fn sample_vm_spec(name: &str) -> VmSpec {
        VmSpec {
            version: 1,
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
            network: Network {
                mode: NetworkMode::User,
            },
            guest: Guest {
                profiles: vec!["default".to_string()],
                capabilities: Capabilities {
                    ssh: true,
                    docker: false,
                    dns: true,
                    forward: true,
                },
            },
            host: Host {
                nested_virtualization: false,
                rosetta: false,
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
    fn inspect_and_list_use_redb_backed_name_and_id_lookup() {
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
}
