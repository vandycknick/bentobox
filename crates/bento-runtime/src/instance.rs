use eyre::Context;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    num::NonZeroI32,
    path::{Path, PathBuf},
};

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
        }
    }
}
