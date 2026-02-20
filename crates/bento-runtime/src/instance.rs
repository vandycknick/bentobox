use eyre::Context;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    num::NonZeroI32,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GuestOs {
    Linux,
    Macos,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EngineType {
    VZ,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiskRole {
    Root,
    Data,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiskConfig {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<DiskRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceDisk {
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Error)]
pub enum InstanceDiskError {
    #[error("only one root disk can be configured, found {count}")]
    MultipleRootDisks { count: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstanceConfig {
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<GuestOs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineType>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_path: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_virtualization: Option<bool>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<DiskConfig>,
}

impl InstanceConfig {
    pub fn new() -> Self {
        let mut i = Self::default();
        i.version = String::from("1.0.0");
        i
    }

    pub fn from_str(input: &str) -> eyre::Result<Self> {
        serde_yaml_ng::from_str(input).context("parse instance config yaml")
    }

    pub fn from_path(path: impl AsRef<Path>) -> eyre::Result<Self> {
        let path = path.as_ref();
        let input = fs::read_to_string(path)
            .wrap_err_with(|| format!("read instance config at {}", path.display()))?;
        Self::from_str(&input)
    }
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub name: String,
    dir: PathBuf,
    pub config: InstanceConfig,
    pub daemon_pid: Option<NonZeroI32>,
}

impl Instance {
    pub fn new(name: String, dir: PathBuf, config: InstanceConfig) -> Self {
        Self {
            name,
            dir,
            config,
            daemon_pid: None,
        }
    }

    pub fn file(&self, f: InstanceFile) -> PathBuf {
        self.dir.join(f.as_str())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn engine(&self) -> EngineType {
        match self.config.engine {
            Some(e) => e,
            None => EngineType::VZ, // TODO: Always default to VZ for now.
        }
    }

    pub fn status(&self) -> InstanceStatus {
        if self.daemon_pid.is_some() {
            InstanceStatus::Running
        } else {
            InstanceStatus::Stopped
        }
    }

    pub fn root_disk(&self) -> Result<Option<InstanceDisk>, InstanceDiskError> {
        let (root, _) = self.partition_disks()?;

        if let Some(root) = root {
            return Ok(Some(self.resolve_config_disk(root)));
        }

        if !self.config.disks.is_empty() {
            return Ok(None);
        }

        let default_root = self.file(InstanceFile::RootDisk);
        let exists = fs::metadata(&default_root)
            .map(|meta| meta.is_file())
            .unwrap_or(false);

        if exists {
            return Ok(Some(InstanceDisk {
                path: default_root,
                read_only: false,
            }));
        }

        Ok(None)
    }

    pub fn data_disks(&self) -> Result<Vec<InstanceDisk>, InstanceDiskError> {
        let (_, data_disks) = self.partition_disks()?;
        Ok(data_disks
            .iter()
            .map(|disk| self.resolve_config_disk(disk))
            .collect())
    }

    fn partition_disks(
        &self,
    ) -> Result<(Option<&DiskConfig>, Vec<&DiskConfig>), InstanceDiskError> {
        let mut root_disk: Option<&DiskConfig> = None;
        let mut root_count = 0usize;
        let mut data_disks = Vec::new();

        for disk in &self.config.disks {
            match disk.role.unwrap_or(DiskRole::Root) {
                DiskRole::Root => {
                    root_count += 1;
                    if root_disk.is_none() {
                        root_disk = Some(disk);
                    }
                }
                DiskRole::Data => data_disks.push(disk),
            }
        }

        if root_count > 1 {
            return Err(InstanceDiskError::MultipleRootDisks { count: root_count });
        }

        Ok((root_disk, data_disks))
    }

    fn resolve_config_disk(&self, disk: &DiskConfig) -> InstanceDisk {
        let path = if disk.path.is_absolute() {
            disk.path.clone()
        } else {
            self.dir.join(&disk.path)
        };

        InstanceDisk {
            path,
            read_only: disk.read_only.unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Unknown,
    Broken,
    Running,
    Stopped,
}

pub enum InstanceFile {
    Config,
    InstancedPid,
    InstancedStdoutLog,
    InstancedSterrLog,
    AppleMachineIdentifier,
    SerialLog,
    RootDisk,
}

impl InstanceFile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Config => "config.yaml",
            Self::InstancedPid => "id.pid",
            Self::InstancedStdoutLog => "id.stdout.log",
            Self::InstancedSterrLog => "id.stder.log",
            Self::AppleMachineIdentifier => "apple-machine-id",
            Self::SerialLog => "serial.log",
            Self::RootDisk => "rootfs.img",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_instance_dir(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("bento-{prefix}-{}-{ts}", std::process::id()))
    }

    #[test]
    fn root_disk_returns_none_when_default_rootfs_is_missing() {
        let mut cfg = InstanceConfig::new();
        cfg.disks = Vec::new();

        let inst = Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), cfg);
        let root = inst.root_disk().expect("root disk lookup should succeed");

        assert!(root.is_none());
    }

    #[test]
    fn root_disk_uses_default_rootfs_when_present() {
        let dir = temp_instance_dir("root-default");
        fs::create_dir_all(&dir).expect("test dir should be creatable");
        fs::write(dir.join(InstanceFile::RootDisk.as_str()), b"disk")
            .expect("root disk file should be creatable");

        let mut cfg = InstanceConfig::new();
        cfg.disks = Vec::new();

        let inst = Instance::new("vm1".to_string(), dir.clone(), cfg);
        let root = inst
            .root_disk()
            .expect("root disk lookup should succeed")
            .expect("root disk should exist");

        assert_eq!(root.path, dir.join("rootfs.img"));
        assert!(!root.read_only);

        fs::remove_dir_all(dir).expect("test dir should be removable");
    }

    #[test]
    fn root_disk_relative_path_resolves_from_instance_dir() {
        let mut cfg = InstanceConfig::new();
        cfg.disks = vec![DiskConfig {
            path: PathBuf::from("images/root.img"),
            role: Some(DiskRole::Root),
            read_only: Some(true),
        }];

        let inst = Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), cfg);
        let root = inst
            .root_disk()
            .expect("root disk should resolve")
            .expect("root disk should be present");

        assert_eq!(root.path, PathBuf::from("/tmp/vm1/images/root.img"));
        assert!(root.read_only);
    }

    #[test]
    fn data_disks_excludes_root() {
        let mut cfg = InstanceConfig::new();
        cfg.disks = vec![
            DiskConfig {
                path: PathBuf::from("root.img"),
                role: Some(DiskRole::Root),
                read_only: None,
            },
            DiskConfig {
                path: PathBuf::from("data.img"),
                role: Some(DiskRole::Data),
                read_only: Some(false),
            },
        ];

        let inst = Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), cfg);
        let data_disks = inst.data_disks().expect("data disks should resolve");

        assert_eq!(data_disks.len(), 1);
        assert_eq!(data_disks[0].path, PathBuf::from("/tmp/vm1/data.img"));
    }

    #[test]
    fn more_than_one_root_disk_returns_error() {
        let mut cfg = InstanceConfig::new();
        cfg.disks = vec![
            DiskConfig {
                path: PathBuf::from("root-a.img"),
                role: Some(DiskRole::Root),
                read_only: None,
            },
            DiskConfig {
                path: PathBuf::from("root-b.img"),
                role: None,
                read_only: None,
            },
        ];

        let inst = Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), cfg);
        let err = inst.root_disk().expect_err("multiple roots must fail");

        assert!(matches!(
            err,
            InstanceDiskError::MultipleRootDisks { count: 2 }
        ));
    }

    #[test]
    fn data_only_disks_return_no_root_disk() {
        let mut cfg = InstanceConfig::new();
        cfg.disks = vec![DiskConfig {
            path: PathBuf::from("data.img"),
            role: Some(DiskRole::Data),
            read_only: None,
        }];

        let inst = Instance::new("vm1".to_string(), PathBuf::from("/tmp/vm1"), cfg);
        let root = inst.root_disk().expect("root disk lookup should succeed");

        assert!(root.is_none());
    }
}
