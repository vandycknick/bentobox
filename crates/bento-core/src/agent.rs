use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const DNS_RECORD_HOST_BENTO_INTERNAL: &str = "host.bento.internal";
pub const RESERVED_SHELL_PORT: u32 = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AgentSshConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentDnsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_dns_listen_address")]
    pub listen_address: IpAddr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_servers: Vec<SocketAddr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<AgentDnsZone>,
}

impl Default for AgentDnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_address: default_dns_listen_address(),
            upstream_servers: Vec::new(),
            zones: Vec::new(),
        }
    }
}

fn default_dns_listen_address() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentDnsZone {
    pub domain: String,
    #[serde(default)]
    pub authoritative: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<AgentDnsRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentDnsRecord {
    pub name: String,
    #[serde(flatten)]
    pub value: AgentDnsRecordValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value", rename_all = "UPPERCASE")]
pub enum AgentDnsRecordValue {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Cname(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AgentConfig {
    #[serde(default)]
    pub ssh: AgentSshConfig,
    #[serde(default)]
    pub dns: AgentDnsConfig,
    #[serde(default)]
    pub forward: AgentForwardConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AgentForwardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub port: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uds: Vec<AgentUdsForwardConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentUdsForwardConfig {
    pub guest_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForwardStreamRequest {
    Api { request: ForwardApiRequest },
    Tcp { guest_port: u16 },
    Uds { guest_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForwardApiRequest {
    ListTcpPorts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForwardApiResponse {
    TcpPorts { ports: Vec<u16> },
    Error { message: String },
}
