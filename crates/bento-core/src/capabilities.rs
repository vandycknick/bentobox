use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const DNS_RECORD_HOST_BENTO_INTERNAL: &str = "host.bento.internal";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SshCapabilityConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DnsCapabilityConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_dns_listen_address")]
    pub listen_address: IpAddr,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_servers: Vec<SocketAddr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<DnsZone>,
}

impl Default for DnsCapabilityConfig {
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
pub struct DnsZone {
    pub domain: String,
    #[serde(default)]
    pub authoritative: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<DnsRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DnsRecord {
    pub name: String,
    #[serde(flatten)]
    pub value: DnsRecordValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value", rename_all = "UPPERCASE")]
pub enum DnsRecordValue {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Cname(String),
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<TcpPortForwardConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TcpPortForwardConfig {
    pub guest_port: u16,
    pub host_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UdsForwardConfig {
    pub name: String,
    pub guest_path: String,
    pub host_path: String,
}
