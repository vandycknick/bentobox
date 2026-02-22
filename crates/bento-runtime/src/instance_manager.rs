use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io,
    os::unix::process::CommandExt,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{self, Signal},
    unistd::{setsid, Pid},
};
use thiserror::Error;

use crate::{
    directories::Directory,
    driver::get_driver_for,
    instance::{Instance, InstanceConfig, InstanceFile, InstanceStatus},
    log_watcher::{LogWatcher, StreamKind, WatchError},
    utils::read_pid_file,
};

pub trait Daemon {
    fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self;
    fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self;
    fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self;
    fn spawn(&mut self) -> io::Result<Child>;
}

pub struct NixDaemon {
    command: Command,
}

impl NixDaemon {
    pub fn new(exe: impl AsRef<OsStr>) -> Self {
        let mut command = Command::new(exe.as_ref());
        unsafe {
            command.pre_exec(|| {
                setsid()
                    .map(|_| ())
                    .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
            });
        }

        Self { command }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.command.arg(arg);
        self
    }
}

impl Daemon for NixDaemon {
    fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdin(cfg);
        self
    }

    fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdout(cfg);
        self
    }

    fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stderr(cfg);
        self
    }

    fn spawn(&mut self) -> io::Result<Child> {
        self.command.spawn()
    }
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

    #[error("generic instance create error")]
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
        config.cpus = Some(options.cpus as i32);
        config.memory = Some(options.memory as i32);
        config.kernel_path = options
            .kernel_path
            .map(|path| path_relative_to(&app_home, &path));

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

    pub fn start(&mut self, inst: &Instance) -> Result<(), InstanceError> {
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

        wait_for_instanced_start(&pid_path, &stderr_path)?;

        let watcher = LogWatcher::spawn(
            stdout_path,
            stderr_path,
            Duration::from_secs(60 * 10),
            Duration::from_millis(50),
        );

        loop {
            match watcher.recv().map_err(|_| InstanceError::GenericError {
                reason: String::from("failed"),
            })? {
                Ok(line) => {
                    // TODO: I might want to handle this differently
                    if line.stream == StreamKind::Stderr {
                        eprintln!("[instanced] {}", line.text.trim_end());
                        continue;
                    }

                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line.text) {
                        match v.get("type").and_then(|t| t.as_str()) {
                            Some("Running") => return Ok(()),
                            Some("Exiting") => {
                                return Err(InstanceError::GenericError {
                                    reason: "instanced exited before running".to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                }
                Err(WatchError::TimedOut) => {
                    return Err(InstanceError::Io(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for Running event",
                    )));
                }
                Err(WatchError::Io(err)) => return Err(err.into()),
            }
        }
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
    pub kernel_path: Option<PathBuf>,
}

impl Default for InstanceCreateOptions {
    fn default() -> Self {
        Self {
            cpus: 1,
            memory: 512,
            kernel_path: None,
        }
    }
}

impl InstanceCreateOptions {
    pub fn with_cpus(mut self, cpus: u8) -> Self {
        self.cpus = cpus;
        self
    }

    pub fn with_kernel(mut self, path: Option<PathBuf>) -> Self {
        self.kernel_path = path;
        self
    }

    pub fn with_memory(mut self, memory: u32) -> Self {
        self.memory = memory;
        self
    }
}

fn path_relative_to(base: &Path, target: &Path) -> PathBuf {
    // Return a path from `base` to `target`.
    // If either path is non-absolute, keep `target` unchanged.
    if !base.is_absolute() || !target.is_absolute() {
        return target.to_path_buf();
    }

    let base_components: Vec<Component<'_>> = base.components().collect();
    let target_components: Vec<Component<'_>> = target.components().collect();

    // Find the longest shared prefix between both absolute paths.
    let mut common_len = 0usize;
    while common_len < base_components.len()
        && common_len < target_components.len()
        && base_components[common_len] == target_components[common_len]
    {
        common_len += 1;
    }

    let mut relative = PathBuf::new();

    // For each remaining normal segment in `base`, go up one directory.
    for comp in &base_components[common_len..] {
        if matches!(comp, Component::Normal(_)) {
            relative.push("..");
        }
    }

    // Then descend into the remaining suffix of `target`.
    for comp in &target_components[common_len..] {
        relative.push(comp.as_os_str());
    }

    // Use "." as the canonical relative path when both inputs are identical.
    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
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

pub fn wait_for_instanced_start(ha_pid_path: &Path, ha_stderr_path: &Path) -> io::Result<()> {
    let deadline_duration = Duration::from_secs(5);
    let deadline = Instant::now() + deadline_duration;
    let poll_interval = Duration::from_millis(50);

    loop {
        match std::fs::metadata(ha_pid_path) {
            Ok(_) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        if Instant::now() >= deadline {
            println!("over deadline");
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "hostagent ({}) did not start up in {:?} (hint: see {})",
                    ha_pid_path.display(),
                    deadline_duration,
                    ha_stderr_path.display(),
                ),
            ));
        }

        thread::sleep(poll_interval);
    }
}
