use std::path::PathBuf;

use eyre::Context;
use serde::Deserialize;

use crate::capabilities::{CapabilitiesConfig, UdsForwardConfig};
use crate::directories::Directory;

pub const ENDPOINT_DOCKER: &str = "docker";
pub const ENDPOINT_SSH: &str = "ssh";
pub const ENDPOINT_SERIAL: &str = "serial";

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

pub fn profiles_dir() -> eyre::Result<PathBuf> {
    Directory::with_prefix("")
        .get_config_home()
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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CapabilitiesOverlay {
    #[serde(default)]
    pub ssh: SshCapabilityOverlay,
    #[serde(default)]
    pub dns: DnsCapabilityOverlay,
    #[serde(default)]
    pub forward: ForwardCapabilityOverlay,
}

impl CapabilitiesOverlay {
    pub fn is_empty(&self) -> bool {
        self.ssh.enabled.is_none()
            && self.dns.enabled.is_none()
            && self.dns.aliases.is_empty()
            && self.forward.enabled.is_none()
            && self.forward.tcp.auto_discover.is_none()
            && self.forward.uds.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SshCapabilityOverlay {
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DnsCapabilityOverlay {
    pub enabled: Option<bool>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ForwardCapabilityOverlay {
    pub enabled: Option<bool>,
    #[serde(default)]
    pub tcp: TcpForwardOverlay,
    #[serde(default)]
    pub uds: Vec<UdsForwardConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TcpForwardOverlay {
    pub auto_discover: Option<bool>,
}

impl CapabilitiesConfig {
    pub fn merge(&mut self, overlay: CapabilitiesOverlay) {
        if let Some(enabled) = overlay.ssh.enabled {
            self.ssh.enabled = enabled;
        }

        if let Some(enabled) = overlay.dns.enabled {
            self.dns.enabled = enabled;
        }
        for alias in overlay.dns.aliases {
            if !self.dns.aliases.iter().any(|existing| existing == &alias) {
                self.dns.aliases.push(alias);
            }
        }

        if let Some(enabled) = overlay.forward.enabled {
            self.forward.enabled = enabled;
        }
        if let Some(auto_discover) = overlay.forward.tcp.auto_discover {
            self.forward.tcp.auto_discover = auto_discover;
        }
        for forward in overlay.forward.uds {
            if let Some(existing) = self
                .forward
                .uds
                .iter_mut()
                .find(|existing| existing.name == forward.name)
            {
                *existing = forward;
            } else {
                self.forward.uds.push(forward);
            }
        }
    }
}

pub fn validate_capabilities(capabilities: &CapabilitiesConfig) -> eyre::Result<()> {
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{ForwardCapabilityConfig, TcpForwardConfig};

    #[test]
    fn merge_overlay_updates_scalars_and_appends_lists() {
        let mut capabilities = CapabilitiesConfig::default();
        capabilities.merge(CapabilitiesOverlay {
            ssh: SshCapabilityOverlay {
                enabled: Some(false),
            },
            dns: DnsCapabilityOverlay {
                enabled: None,
                aliases: vec![String::from("host.docker.internal")],
            },
            forward: ForwardCapabilityOverlay {
                enabled: Some(true),
                tcp: TcpForwardOverlay {
                    auto_discover: Some(true),
                },
                uds: vec![UdsForwardConfig {
                    name: String::from("docker"),
                    guest_path: String::from("/var/run/docker.sock"),
                    host_path: String::from("docker.sock"),
                }],
            },
        });

        assert!(!capabilities.ssh.enabled);
        assert!(capabilities
            .dns
            .aliases
            .iter()
            .any(|alias| alias == "host.docker.internal"));
        assert!(capabilities.forward.enabled);
        assert!(capabilities.forward.tcp.auto_discover);
        assert_eq!(capabilities.forward.uds.len(), 1);
    }

    #[test]
    fn validate_capabilities_rejects_empty_uds_name() {
        let capabilities = CapabilitiesConfig {
            forward: ForwardCapabilityConfig {
                enabled: true,
                tcp: TcpForwardConfig::default(),
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
