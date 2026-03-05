use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use thiserror::Error;

use crate::{
    cidata,
    directories::Directory,
    driver::get_driver_for,
    host_user,
    images::capabilities::GuestCapabilities,
    instance::{
        resolve_mount_location, validate_network_mode, DiskConfig, DiskRole, Instance,
        InstanceConfig, InstanceFile, InstanceStatus, MountConfig, NetworkConfig,
    },
    service_readiness, ssh_keys,
    utils::read_pid_file,
};

pub trait Daemon {
    fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self;
    fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self;
    fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self;
    fn spawn(&mut self) -> io::Result<Child>;
}

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error("invalid instance name {name:?}: {reason}")]
    InvalidName { name: String, reason: String },

    #[error("instance {name:?} does not exist ({path})")]
    InstanceNotFound { name: String, path: PathBuf },

    #[error("instance {name:?} path is not a directory ({path})")]
    InstancePathNotADirectory { name: String, path: PathBuf },

    #[error("instance {name:?} already exists")]
    InstanceAlreadyCreated { name: String },

    #[error("instance {name:?} is running")]
    InstanceAlreadyRunning { name: String },

    #[error("instance {name:?} is not running")]
    InstanceNotRunning { name: String },

    #[error("failed to serialize config for instance {name:?}")]
    ConfigSerializeFailed {
        name: String,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("failed to load config for instance {name:?} from path: ({path})")]
    ConfigLoadFailed {
        name: String,
        path: PathBuf,
        #[source]
        source: eyre::Report,
    },

    #[error("generic instance create error: {reason}")]
    GenericError { reason: String },

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Nix(#[from] nix::errno::Errno),

    #[error(transparent)]
    Driver(#[from] crate::driver::DriverError),
}

pub struct InstanceManager<D: Daemon> {
    instanced: D,
}

impl<D: Daemon> InstanceManager<D> {
    pub fn new(daemon: D) -> Self {
        Self { instanced: daemon }
    }

    pub fn create(
        &self,
        name: &str,
        options: impl Into<InstanceCreateOptions>,
    ) -> Result<Instance, InstanceError> {
        validate_name(name)?;

        match self.inspect(name) {
            Ok(_) => {
                return Err(InstanceError::InstanceAlreadyCreated {
                    name: name.to_owned(),
                });
            }
            Err(InstanceError::InstanceNotFound { .. }) => {
                // NOTE: Expected on create path, continue.
            }
            Err(err) => return Err(err),
        }

        let dirs = Directory::with_prefix(name);

        let app_home = dirs
            .get_data_home()
            .ok_or_else(|| InstanceError::GenericError {
                reason:
                    "users data home from $XDG_DATA_HOME or $HOME/.local/share can't be located"
                        .to_string(),
            })?;
        fs::create_dir_all(&app_home)?;

        // TODO: Find a better way to build an InstanceConfig from options.
        let options = options.into();
        let mut config = InstanceConfig::new();
        apply_create_options(&mut config, options)?;
        validate_network_mode(
            config.engine.unwrap_or(crate::instance::EngineType::VZ),
            config.network.as_ref(),
        )
        .map_err(|reason| InstanceError::GenericError { reason })?;

        let config_path = app_home.join(InstanceFile::Config.as_str());
        let config_yaml = serde_yaml_ng::to_string(&config).map_err(|source| {
            InstanceError::ConfigSerializeFailed {
                name: name.to_owned(),
                source,
            }
        })?;
        fs::write(&config_path, config_yaml)?;

        let inst = self.inspect(name)?;
        let driver = get_driver_for(&inst)?;

        driver.validate()?;
        driver.create()?;

        let should_inject_cidata = inst.expects_guest_agent();

        if should_inject_cidata {
            let host_user =
                host_user::current_host_user().map_err(|err| InstanceError::GenericError {
                    reason: format!("resolve current host user failed: {err}"),
                })?;
            let user_keys =
                ssh_keys::ensure_user_ssh_keys().map_err(|err| InstanceError::GenericError {
                    reason: format!("ensure user SSH keys failed: {err}"),
                })?;

            cidata::build_cidata_iso(&inst, &host_user, &user_keys.public_key_openssh).map_err(
                |err| InstanceError::GenericError {
                    reason: format!("build cidata ISO failed: {err}"),
                },
            )?;
        }

        Ok(inst)
    }

    pub fn inspect(&self, name: &str) -> Result<Instance, InstanceError> {
        validate_name(name)?;

        let dirs = Directory::with_prefix(name);
        let app_home = dirs
            .get_data_home()
            .ok_or_else(|| InstanceError::GenericError {
                reason:
                    "Users data home from $XDG_DATA_HOME or $HOME/.local/share can't be located"
                        .to_string(),
            })?;

        match fs::metadata(&app_home) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(InstanceError::InstancePathNotADirectory {
                    name: name.to_string(),
                    path: app_home.to_path_buf(),
                })
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(InstanceError::InstanceNotFound {
                    name: name.to_string(),
                    path: app_home.to_path_buf(),
                })
            }
            Err(_) => {
                return Err(InstanceError::GenericError {
                    reason: "can't stat folder".to_string(),
                })
            }
        }

        let config_path = app_home.join(InstanceFile::Config.as_str());

        let config = InstanceConfig::from_path(&config_path).map_err(|source| {
            InstanceError::ConfigLoadFailed {
                name: name.to_string(),
                path: config_path.clone(),
                source,
            }
        })?;

        let mut inst = Instance::new(name.into(), app_home, config);

        inst.daemon_pid = read_pid_file(&inst.file(InstanceFile::InstancedPid))?;

        Ok(inst)
    }

    pub fn list(&self) -> Result<Vec<Instance>, InstanceError> {
        let root = Directory::with_prefix("").get_data_home().ok_or_else(|| {
            InstanceError::GenericError {
                reason:
                    "Users data home from $XDG_DATA_HOME or $HOME/.local/share can't be located"
                        .to_string(),
            }
        })?;

        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        let mut instances = Vec::new();

        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() {
                continue;
            }

            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };

            if let Ok(instance) = self.inspect(&name) {
                instances.push(instance);
            }
        }

        instances.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(instances)
    }

    pub async fn start(&mut self, inst: &Instance) -> Result<(), InstanceError> {
        inst.validate_network_mode()
            .map_err(|reason| InstanceError::GenericError { reason })?;

        if inst.status() == InstanceStatus::Running {
            return Err(InstanceError::InstanceAlreadyRunning {
                name: inst.name.clone(),
            });
        }

        let pid_path = inst.file(InstanceFile::InstancedPid);
        let stdout_path = inst.file(InstanceFile::InstancedStdoutLog);
        let stderr_path = inst.file(InstanceFile::InstancedSterrLog);

        let stdout = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&stdout_path)?;

        let stderr = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&stderr_path)?;

        self.instanced
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()?;

        wait_for_instanced_start(&pid_path, &stderr_path).await?;

        if inst.expects_guest_agent() {
            service_readiness::wait_for_guest_running(
                &inst.file(InstanceFile::InstancedSocket),
                Duration::from_secs(60 * 10),
            )
            .await
            .map_err(|err| InstanceError::GenericError {
                reason: format!("guest service discovery readiness check failed: {err}"),
            })?;
        }

        Ok(())
    }

    pub fn stop(&self, inst: &Instance) -> Result<(), InstanceError> {
        let daemon_pid = inst
            .daemon_pid
            .ok_or_else(|| InstanceError::InstanceNotRunning {
                name: inst.name.clone(),
            })?;

        let pid = Pid::from_raw(daemon_pid.get());
        signal::kill(pid, Signal::SIGINT)?;

        println!("Send signal to {}", pid);

        Ok(())
    }

    pub fn delete(&self, inst: &Instance) -> Result<(), InstanceError> {
        if inst.status() == InstanceStatus::Running {
            return Err(InstanceError::InstanceAlreadyRunning {
                name: inst.name.clone(),
            });
        }

        match fs::remove_dir_all(inst.dir()) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct InstanceCreateOptions {
    pub cpus: u8,
    pub memory: u32,
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    pub disks: Vec<PathBuf>,
    pub mounts: Vec<MountConfig>,
    pub network: Option<NetworkConfig>,
    pub capabilities: GuestCapabilities,
    pub userdata_path: Option<PathBuf>,
}

impl Default for InstanceCreateOptions {
    fn default() -> Self {
        Self {
            cpus: 1,
            memory: 512,
            kernel: None,
            initramfs: None,
            disks: Vec::new(),
            mounts: Vec::new(),
            network: None,
            capabilities: GuestCapabilities::default(),
            userdata_path: None,
        }
    }
}

impl InstanceCreateOptions {
    pub fn with_cpus(mut self, cpus: u8) -> Self {
        self.cpus = cpus;
        self
    }

    pub fn with_kernel(mut self, path: Option<PathBuf>) -> Self {
        self.kernel = path;
        self
    }

    pub fn with_initramfs(mut self, path: Option<PathBuf>) -> Self {
        self.initramfs = path;
        self
    }

    pub fn with_disks(mut self, disks: Vec<PathBuf>) -> Self {
        self.disks = disks;
        self
    }

    pub fn with_memory(mut self, memory: u32) -> Self {
        self.memory = memory;
        self
    }

    pub fn with_mounts(mut self, mounts: Vec<MountConfig>) -> Self {
        self.mounts = mounts;
        self
    }

    pub fn with_network(mut self, network: Option<NetworkConfig>) -> Self {
        self.network = network;
        self
    }

    pub fn with_capabilities(mut self, capabilities: GuestCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_userdata(mut self, userdata_path: Option<PathBuf>) -> Self {
        self.userdata_path = userdata_path;
        self
    }
}

fn apply_create_options(
    config: &mut InstanceConfig,
    options: InstanceCreateOptions,
) -> Result<(), InstanceError> {
    config.cpus = Some(options.cpus as i32);
    config.memory = Some(options.memory as i32);
    config.kernel_path = options.kernel;
    config.initramfs_path = options.initramfs;
    config.disks = options
        .disks
        .iter()
        .map(|path| DiskConfig {
            path: path.clone(),
            role: Some(DiskRole::Data),
            read_only: Some(false),
        })
        .collect();
    config.mounts = normalize_mounts(&options.mounts)?;
    config.network = options.network;
    config.capabilities = options.capabilities;
    config.userdata_path = options.userdata_path;

    Ok(())
}

fn normalize_mounts(mounts: &[MountConfig]) -> Result<Vec<MountConfig>, InstanceError> {
    if mounts.is_empty() {
        return Ok(Vec::new());
    }

    let cwd = std::env::current_dir().map_err(|err| InstanceError::GenericError {
        reason: format!("resolve current working directory failed: {err}"),
    })?;

    let mut normalized = Vec::with_capacity(mounts.len());
    let mut seen = std::collections::HashSet::with_capacity(mounts.len());

    for mount in mounts {
        let preserve_tilde = is_tilde_mount_path(&mount.location)?;
        let runtime_path = resolve_mount_location(&mount.location).map_err(|reason| {
            InstanceError::GenericError {
                reason: format!(
                    "invalid mount location {}: {reason}",
                    mount.location.display()
                ),
            }
        })?;

        let absolute = if runtime_path.is_absolute() {
            runtime_path
        } else {
            cwd.join(&runtime_path)
        };

        let canonical =
            std::fs::canonicalize(&absolute).map_err(|err| InstanceError::GenericError {
                reason: format!(
                    "mount location {} does not exist: {err}",
                    absolute.display()
                ),
            })?;

        let metadata =
            std::fs::metadata(&canonical).map_err(|err| InstanceError::GenericError {
                reason: format!(
                    "inspect mount location {} failed: {err}",
                    canonical.display()
                ),
            })?;
        if !metadata.is_dir() {
            return Err(InstanceError::GenericError {
                reason: format!("mount location is not a directory: {}", canonical.display()),
            });
        }

        if !seen.insert(canonical.clone()) {
            return Err(InstanceError::GenericError {
                reason: format!("duplicate mount location: {}", canonical.display()),
            });
        }

        normalized.push(MountConfig {
            location: if preserve_tilde {
                mount.location.clone()
            } else {
                canonical
            },
            writable: mount.writable,
        });
    }

    Ok(normalized)
}

fn is_tilde_mount_path(path: &Path) -> Result<bool, InstanceError> {
    let path = path.to_string_lossy();
    if path == "~" || path.starts_with("~/") {
        return Ok(true);
    }

    if path.starts_with('~') {
        return Err(InstanceError::GenericError {
            reason: format!(
                "invalid mount path '{}': only '~' and '~/...' are supported",
                path
            ),
        });
    }

    Ok(false)
}

pub fn validate_name(name: &str) -> Result<(), InstanceError> {
    if name.is_empty() {
        return Err(InstanceError::InvalidName {
            name: name.to_owned(),
            reason: "empty instance name".into(),
        });
    }

    if let Some(ch) = name
        .chars()
        .find(|ch| !ch.is_ascii_alphanumeric() && *ch != '-' && *ch != '_')
    {
        return Err(InstanceError::InvalidName {
            name: name.to_owned(),
            reason: format!("invalid character: {ch:?}"),
        });
    }

    Ok(())
}

pub async fn wait_for_instanced_start(ha_pid_path: &Path, ha_stderr_path: &Path) -> io::Result<()> {
    let deadline_duration = Duration::from_secs(5);
    let deadline = Instant::now() + deadline_duration;
    let poll_interval = Duration::from_millis(50);

    loop {
        match tokio::fs::metadata(ha_pid_path).await {
            Ok(_) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "instanced ({}) did not start up in {:?} (hint: see {})",
                    ha_pid_path.display(),
                    deadline_duration,
                    ha_stderr_path.display(),
                ),
            ));
        }

        tokio::time::sleep(poll_interval).await;
    }
}
