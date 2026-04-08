use serde::{Deserialize, Serialize};

use crate::capabilities::DnsCapabilityConfig;

pub const DEFAULT_AGENT_CONTROL_PORT: u32 = 1027;
pub const RESERVED_SSH_PORT: u32 = 2000;
pub const RESERVED_FORWARD_PORT_START: u32 = 2001;
pub const SERVICE_ID_SSH: &str = "ssh";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestRuntimeConfig {
    pub control_port: u32,
    #[serde(default)]
    pub dns: DnsCapabilityConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<GuestServiceConfig>,
}

impl Default for GuestRuntimeConfig {
    fn default() -> Self {
        Self {
            control_port: DEFAULT_AGENT_CONTROL_PORT,
            dns: DnsCapabilityConfig::default(),
            services: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuestServiceKind {
    Ssh,
    UnixSocketForward,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestServiceConfig {
    pub id: String,
    pub kind: GuestServiceKind,
    pub port: u32,
    #[serde(default)]
    pub startup_required: bool,
    pub target: String,
}
