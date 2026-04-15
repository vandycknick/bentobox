use std::path::PathBuf;

use bento_core::capabilities::{CapabilitiesConfig, CapabilitiesOverlay, DnsRecordValue};
use eyre::Context;
use serde::Deserialize;

use crate::layout::resolve_config_dir;

pub fn resolve_profiles(
    base: &CapabilitiesConfig,
    profiles: &[String],
) -> eyre::Result<CapabilitiesConfig> {
    let mut capabilities = base.clone();
    for profile in profiles {
        let overlay = load_profile(profile)?;
        capabilities.merge(overlay);
    }
    Ok(capabilities)
}

pub fn validate_capabilities(capabilities: &CapabilitiesConfig) -> eyre::Result<()> {
    for zone in &capabilities.dns.zones {
        if zone.domain.trim().is_empty() {
            eyre::bail!("dns zone domain cannot be empty");
        }

        for record in &zone.records {
            if record.name.trim().is_empty() {
                eyre::bail!("dns record name in zone '{}' cannot be empty", zone.domain);
            }

            if matches!(&record.value, DnsRecordValue::Cname(target) if target.trim().is_empty()) {
                eyre::bail!(
                    "dns CNAME record '{}' in zone '{}' cannot have an empty target",
                    record.name,
                    zone.domain
                );
            }
        }
    }

    for forward in &capabilities.forward.uds {
        if forward.name.trim().is_empty() {
            eyre::bail!("forward uds entry name cannot be empty");
        }
        if forward.guest_path.trim().is_empty() {
            eyre::bail!(
                "forward uds entry '{}' guest_path cannot be empty",
                forward.name
            );
        }
        if forward.host_path.trim().is_empty() {
            eyre::bail!(
                "forward uds entry '{}' host_path cannot be empty",
                forward.name
            );
        }
    }

    for mapping in &capabilities.forward.tcp.ports {
        if mapping.guest_port == 0 {
            eyre::bail!("forward tcp guest_port cannot be zero");
        }
        if mapping.host_port == 0 {
            eyre::bail!("forward tcp host_port cannot be zero");
        }
    }

    Ok(())
}

fn profiles_dir() -> eyre::Result<PathBuf> {
    resolve_config_dir()
        .map(|base| base.join("profiles"))
        .ok_or_else(|| eyre::eyre!("resolve ~/.config/bento/profiles path"))
}

fn load_profile(profile: &str) -> eyre::Result<CapabilitiesOverlay> {
    let path = profiles_dir()?.join(format!("{profile}.yaml"));
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read profile {}", path.display()))?;
    let parsed: ProfileFile = serde_yaml_ng::from_str(&raw)
        .with_context(|| format!("parse profile {}", path.display()))?;
    Ok(parsed.capabilities)
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ProfileFile {
    #[serde(default)]
    capabilities: CapabilitiesOverlay,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bento_core::capabilities::{
        CapabilitiesOverlay, DnsCapabilityOverlay, DnsRecord, DnsRecordValue, DnsZone,
        ForwardCapabilityConfig, ForwardCapabilityOverlay, SshCapabilityOverlay, TcpForwardConfig,
        TcpForwardOverlay, TcpPortForwardConfig, UdsForwardConfig,
    };

    #[test]
    fn merge_overlay_updates_scalars_and_appends_lists() {
        let mut capabilities = CapabilitiesConfig::default();
        capabilities.merge(CapabilitiesOverlay {
            ssh: SshCapabilityOverlay {
                enabled: Some(false),
            },
            dns: DnsCapabilityOverlay {
                enabled: None,
                listen_address: None,
                upstream_servers: vec!["1.1.1.1:53".parse().expect("valid socket addr")],
                zones: vec![DnsZone {
                    domain: String::from("docker.internal"),
                    authoritative: false,
                    records: vec![DnsRecord {
                        name: String::from("host"),
                        value: DnsRecordValue::Cname(String::from("host.bento.internal")),
                    }],
                }],
            },
            forward: ForwardCapabilityOverlay {
                enabled: Some(true),
                tcp: TcpForwardOverlay {
                    auto_discover: Some(true),
                    ports: vec![TcpPortForwardConfig {
                        guest_port: 8080,
                        host_port: 8080,
                    }],
                },
                uds: vec![UdsForwardConfig {
                    name: String::from("docker"),
                    guest_path: String::from("/var/run/docker.sock"),
                    host_path: String::from("docker.sock"),
                }],
            },
        });

        assert!(!capabilities.ssh.enabled);
        assert_eq!(capabilities.dns.upstream_servers.len(), 1);
        assert_eq!(capabilities.dns.zones.len(), 1);
        assert_eq!(capabilities.dns.zones[0].domain, "docker.internal");
        assert!(capabilities.forward.enabled);
        assert!(capabilities.forward.tcp.auto_discover);
        assert_eq!(capabilities.forward.tcp.ports.len(), 1);
        assert_eq!(capabilities.forward.uds.len(), 1);
    }

    #[test]
    fn validate_capabilities_allows_dns_without_upstreams() {
        let capabilities = CapabilitiesConfig::default();
        assert!(validate_capabilities(&capabilities).is_ok());
    }

    #[test]
    fn validate_capabilities_rejects_empty_uds_name() {
        let capabilities = CapabilitiesConfig {
            forward: ForwardCapabilityConfig {
                enabled: true,
                tcp: TcpForwardConfig {
                    auto_discover: false,
                    ports: Vec::new(),
                },
                uds: vec![UdsForwardConfig {
                    name: String::new(),
                    guest_path: String::from("/tmp/guest.sock"),
                    host_path: String::from("guest.sock"),
                }],
            },
            ..CapabilitiesConfig::default()
        };

        assert!(validate_capabilities(&capabilities).is_err());
    }
}
