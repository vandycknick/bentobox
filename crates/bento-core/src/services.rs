use serde::{Deserialize, Serialize};

use crate::capabilities::{DnsCapabilityConfig, SshCapabilityConfig};

pub const RESERVED_SHELL_PORT: u32 = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestRuntimeConfig {
    #[serde(default)]
    pub ssh: SshCapabilityConfig,
    #[serde(default)]
    pub dns: DnsCapabilityConfig,
    #[serde(default)]
    pub forward: GuestForwardConfig,
}

impl Default for GuestRuntimeConfig {
    fn default() -> Self {
        Self {
            ssh: SshCapabilityConfig::default(),
            dns: DnsCapabilityConfig::default(),
            forward: GuestForwardConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GuestForwardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub port: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uds: Vec<GuestUdsForwardConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestUdsForwardConfig {
    pub guest_path: String,
}
