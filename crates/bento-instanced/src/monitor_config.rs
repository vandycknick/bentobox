use std::path::{Path, PathBuf};

use bento_core::capabilities::{
    CapabilitiesConfig, DnsCapabilityConfig, ForwardCapabilityConfig, SshCapabilityConfig,
};
use bento_core::{InstanceFile, NetworkMode as SpecNetworkMode, VmSpec};

#[derive(Debug, Clone)]
pub(crate) struct VmContext {
    pub name: String,
    pub data_dir: PathBuf,
    pub spec: VmSpec,
}

#[derive(Debug, Clone)]
pub(crate) struct MonitorMount {
    pub source: PathBuf,
    pub tag: String,
    pub writable: bool,
}

impl VmContext {
    pub(crate) fn dir(&self) -> &Path {
        &self.data_dir
    }

    pub(crate) fn file(&self, file: InstanceFile) -> PathBuf {
        self.data_dir.join(file.as_str())
    }

    pub(crate) fn base_capabilities(&self) -> CapabilitiesConfig {
        CapabilitiesConfig {
            ssh: SshCapabilityConfig {
                enabled: self.spec.guest.capabilities.ssh,
            },
            dns: DnsCapabilityConfig {
                enabled: self.spec.guest.capabilities.dns,
                ..DnsCapabilityConfig::default()
            },
            forward: ForwardCapabilityConfig {
                enabled: self.spec.guest.capabilities.forward,
                ..ForwardCapabilityConfig::default()
            },
        }
    }

    pub(crate) fn profiles(&self) -> &[String] {
        &self.spec.guest.profiles
    }

    pub(crate) fn requires_bootstrap_for(&self, capabilities: &CapabilitiesConfig) -> bool {
        self.spec.boot.bootstrap.is_some()
            || capabilities.requires_bootstrap()
            || self.spec.host.rosetta
    }

    pub(crate) fn resolved_network_mode(&self) -> bento_vmm::NetworkMode {
        match self.spec.network.mode {
            SpecNetworkMode::None => bento_vmm::NetworkMode::None,
            SpecNetworkMode::User => bento_vmm::NetworkMode::VzNat,
            SpecNetworkMode::Bridged => bento_vmm::NetworkMode::Bridged,
        }
    }

    pub(crate) fn rosetta_enabled(&self) -> bool {
        self.spec.host.rosetta
    }

    pub(crate) fn userdata_path(&self) -> Option<&Path> {
        self.spec
            .boot
            .bootstrap
            .as_ref()
            .and_then(|bootstrap| bootstrap.cloud_init.as_deref())
    }

    pub(crate) fn mounts(&self) -> Vec<MonitorMount> {
        self.spec
            .mounts
            .iter()
            .map(|mount| MonitorMount {
                source: mount.source.clone(),
                tag: mount.tag.clone(),
                writable: !mount.read_only,
            })
            .collect()
    }
}
