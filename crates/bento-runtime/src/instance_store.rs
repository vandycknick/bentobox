use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::{
    directories::Directory,
    extensions::ExtensionsConfig,
    instance::{
        resolve_mount_location, validate_network_mode, BootstrapConfig, DiskConfig, DiskRole,
        Instance, InstanceConfig, InstanceFile, InstanceStatus, MountConfig, NetworkConfig,
    },
    utils::read_pid_file,
};

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
}

pub struct InstanceStore;

#[derive(Debug)]
pub struct PendingInstance {
    instance: Instance,
    final_dir: PathBuf,
    committed: bool,
}

impl Default for InstanceStore {
    fn default() -> Self {
        Self
    }
}

impl InstanceStore {
    pub fn new() -> Self {
        Self
    }

    pub fn create(
        &self,
        name: &str,
        options: impl Into<InstanceCreateOptions>,
    ) -> Result<Instance, InstanceError> {
        self.create_pending(name, options)?.commit()
    }

    pub fn create_pending(
        &self,
        name: &str,
        options: impl Into<InstanceCreateOptions>,
    ) -> Result<PendingInstance, InstanceError> {
        validate_name(name)?;

        let data_root = Directory::with_prefix("").get_data_home().ok_or_else(|| {
            InstanceError::GenericError {
                reason:
                    "users data home from $XDG_DATA_HOME or $HOME/.local/share can't be located"
                        .to_string(),
            }
        })?;

        // TODO: Find a better way to build an InstanceConfig from options.
        let options = options.into();
        let mut config = InstanceConfig::new();
        apply_create_options(&mut config, options)?;
        validate_network_mode(
            config
                .engine
                .unwrap_or_else(crate::instance::default_engine_type),
            config.network.as_ref(),
        )
        .map_err(|reason| InstanceError::GenericError { reason })?;

        let config_yaml = serde_yaml_ng::to_string(&config).map_err(|source| {
            InstanceError::ConfigSerializeFailed {
                name: name.to_owned(),
                source,
            }
        })?;

        let final_dir = data_root.join(name);
        ensure_instance_path_available(name, &final_dir)?;

        fs::create_dir_all(&data_root)?;
        let staged_dir = create_staging_dir(&data_root, name)?;

        let config_path = staged_dir.join(InstanceFile::Config.as_str());
        if let Err(err) = fs::write(&config_path, config_yaml) {
            let _ = fs::remove_dir_all(&staged_dir);
            return Err(err.into());
        }

        Ok(PendingInstance {
            instance: Instance::new(name.to_owned(), staged_dir, config),
            final_dir,
            committed: false,
        })
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

impl PendingInstance {
    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn commit(mut self) -> Result<Instance, InstanceError> {
        ensure_instance_path_available(&self.instance.name, &self.final_dir)?;
        fs::rename(self.instance.dir(), &self.final_dir)?;

        self.committed = true;

        Ok(Instance::new(
            self.instance.name.clone(),
            self.final_dir.clone(),
            self.instance.config.clone(),
        ))
    }
}

impl Drop for PendingInstance {
    fn drop(&mut self) {
        if self.committed {
            return;
        }

        match fs::remove_dir_all(self.instance.dir()) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

#[derive(Debug)]
pub struct InstanceCreateOptions {
    pub cpus: u8,
    pub memory: u32,
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    pub nested_virtualization: bool,
    pub rosetta: bool,
    pub disks: Vec<PathBuf>,
    pub mounts: Vec<MountConfig>,
    pub network: Option<NetworkConfig>,
    pub bootstrap: Option<BootstrapConfig>,
    pub extensions: ExtensionsConfig,
    pub userdata_path: Option<PathBuf>,
}

impl Default for InstanceCreateOptions {
    fn default() -> Self {
        Self {
            cpus: 1,
            memory: 512,
            kernel: None,
            initramfs: None,
            nested_virtualization: false,
            rosetta: false,
            disks: Vec::new(),
            mounts: Vec::new(),
            network: None,
            bootstrap: None,
            extensions: ExtensionsConfig::default(),
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

    pub fn with_nested_virtualization(mut self, enabled: bool) -> Self {
        self.nested_virtualization = enabled;
        self
    }

    pub fn with_rosetta(mut self, enabled: bool) -> Self {
        self.rosetta = enabled;
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

    pub fn with_bootstrap(mut self, bootstrap: Option<BootstrapConfig>) -> Self {
        self.bootstrap = bootstrap;
        self
    }

    pub fn with_extensions(mut self, extensions: ExtensionsConfig) -> Self {
        self.extensions = extensions;
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
    config.nested_virtualization = Some(options.nested_virtualization);
    config.rosetta = Some(options.rosetta);
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
    config.bootstrap = options.bootstrap;
    config.extensions = options.extensions;
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

fn ensure_instance_path_available(name: &str, path: &Path) -> Result<(), InstanceError> {
    match fs::metadata(path) {
        Ok(_) => Err(InstanceError::InstanceAlreadyCreated {
            name: name.to_owned(),
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn create_staging_dir(data_root: &Path, name: &str) -> Result<PathBuf, InstanceError> {
    let staging_root = data_root.join(".staging");
    fs::create_dir_all(&staging_root)?;

    for attempt in 0..256u32 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| InstanceError::GenericError {
                reason: format!("system clock error while creating staging dir: {err}"),
            })?
            .as_nanos();
        let candidate = staging_root.join(format!(
            "{name}-{}-{timestamp}-{attempt}",
            std::process::id()
        ));

        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }

    Err(InstanceError::GenericError {
        reason: format!("failed to allocate staging directory for instance {name:?}"),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "bento-instance-store-{prefix}-{}-{timestamp}",
            std::process::id()
        ))
    }

    fn with_test_data_home<T>(prefix: &str, test: impl FnOnce(&Path) -> T) -> T {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let root = unique_test_dir(prefix);
        fs::create_dir_all(&root).expect("test data root should be creatable");

        let original = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &root);
        }

        let result = test(&root);

        match original {
            Some(value) => unsafe { std::env::set_var("XDG_DATA_HOME", value) },
            None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
        }

        fs::remove_dir_all(&root).expect("test data root should be removable");
        result
    }

    #[test]
    fn apply_create_options_persists_nested_virtualization() {
        let mut config = InstanceConfig::new();

        apply_create_options(
            &mut config,
            InstanceCreateOptions::default().with_nested_virtualization(true),
        )
        .expect("apply create options should succeed");

        assert_eq!(config.nested_virtualization, Some(true));
    }

    #[test]
    fn apply_create_options_persists_rosetta() {
        let mut config = InstanceConfig::new();

        apply_create_options(
            &mut config,
            InstanceCreateOptions::default().with_rosetta(true),
        )
        .expect("apply create options should succeed");

        assert_eq!(config.rosetta, Some(true));
    }

    #[test]
    fn pending_instance_drop_removes_staged_directory() {
        with_test_data_home("pending-drop", |_| {
            let store = InstanceStore::new();
            let pending = store
                .create_pending("vm-drop", InstanceCreateOptions::default())
                .expect("pending instance should be created");

            let staged_dir = pending.instance().dir().to_path_buf();
            assert!(staged_dir.exists());

            drop(pending);

            assert!(!staged_dir.exists());
            assert!(!Directory::with_prefix("vm-drop")
                .get_data_home()
                .expect("final dir should resolve")
                .exists());
        });
    }

    #[test]
    fn pending_instance_commit_promotes_staged_directory() {
        with_test_data_home("pending-commit", |_| {
            let store = InstanceStore::new();
            let pending = store
                .create_pending("vm-commit", InstanceCreateOptions::default())
                .expect("pending instance should be created");

            let staged_dir = pending.instance().dir().to_path_buf();
            let committed = pending.commit().expect("commit should succeed");

            assert!(!staged_dir.exists());
            assert_eq!(
                committed.dir(),
                Directory::with_prefix("vm-commit")
                    .get_data_home()
                    .expect("final dir should resolve")
            );
            assert!(committed.file(InstanceFile::Config).is_file());
        });
    }
}
