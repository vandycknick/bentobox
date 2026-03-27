use serde::{Deserialize, Serialize};

pub const CAPABILITY_SSH: &str = "ssh";
pub const CAPABILITY_DNS: &str = "dns";
pub const CAPABILITY_FORWARD: &str = "forward";
pub const DNS_ALIAS_HOST_BENTO_INTERNAL: &str = "host.bento.internal";
pub const DNS_ALIAS_HOST_DOCKER_INTERNAL: &str = "host.docker.internal";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilitiesConfig {
    #[serde(default)]
    pub ssh: SshCapabilityConfig,
    #[serde(default)]
    pub dns: DnsCapabilityConfig,
    #[serde(default)]
    pub forward: ForwardCapabilityConfig,
}

impl Default for CapabilitiesConfig {
    fn default() -> Self {
        Self {
            ssh: SshCapabilityConfig { enabled: true },
            dns: DnsCapabilityConfig {
                enabled: true,
                aliases: vec![String::from(DNS_ALIAS_HOST_BENTO_INTERNAL)],
            },
            forward: ForwardCapabilityConfig::default(),
        }
    }
}

impl CapabilitiesConfig {
    pub fn requires_bootstrap(&self) -> bool {
        self.ssh.enabled || self.dns.enabled || self.forward.enabled
    }

    pub fn startup_required_capabilities(&self) -> Vec<&'static str> {
        let mut capabilities = Vec::new();
        if self.ssh.enabled {
            capabilities.push(CAPABILITY_SSH);
        }
        capabilities
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SshCapabilityConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DnsCapabilityConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ForwardCapabilityConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub tcp: TcpForwardConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uds: Vec<UdsForwardConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TcpForwardConfig {
    #[serde(default)]
    pub auto_discover: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UdsForwardConfig {
    pub name: String,
    pub guest_path: String,
    pub host_path: String,
}
