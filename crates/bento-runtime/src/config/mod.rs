use std::{fs, path::Path};

use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GuestOs {
    Linux,
    Macos,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstanceConfig {
    #[serde(default)]
    pub os: Option<GuestOs>,
    #[serde(default)]
    pub cpus: Option<usize>,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
    #[serde(default)]
    pub console: Option<bool>,
    #[serde(default)]
    pub network: Option<bool>,
    #[serde(default)]
    pub entropy: Option<bool>,
    #[serde(default)]
    pub memory_balloon: Option<bool>,
    #[serde(default)]
    pub graphics: Option<bool>,
    #[serde(default)]
    pub keyboard: Option<bool>,
}

impl InstanceConfig {
    pub fn os(&self) -> GuestOs {
        self.os.unwrap_or(GuestOs::Linux)
    }

    pub fn cpus(&self) -> usize {
        self.cpus.unwrap_or(4)
    }

    pub fn memory_bytes(&self) -> u64 {
        self.memory_bytes.unwrap_or(4 * 1024 * 1024 * 1024)
    }

    pub fn console(&self) -> bool {
        self.console.unwrap_or(false)
    }

    pub fn network(&self) -> bool {
        self.network.unwrap_or(true)
    }

    pub fn entropy(&self) -> bool {
        self.entropy.unwrap_or(true)
    }

    pub fn memory_balloon(&self) -> bool {
        self.memory_balloon.unwrap_or(true)
    }

    pub fn graphics(&self) -> bool {
        self.graphics.unwrap_or(false)
    }

    pub fn keyboard(&self) -> bool {
        self.keyboard.unwrap_or(false)
    }

    pub fn from_str(input: &str) -> Result<Self> {
        serde_yaml_ng::from_str(input).wrap_err("parse instance config yaml")
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let input = fs::read_to_string(path)
            .wrap_err_with(|| format!("read instance config at {}", path.display()))?;
        Self::from_str(&input)
    }
}
