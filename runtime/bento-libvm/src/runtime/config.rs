use std::path::PathBuf;

use crate::network::NetworkDriverKind;
use crate::paths::resolve_default_data_dir;
use crate::LibVmError;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub(crate) target: RuntimeTarget,
}

impl RuntimeConfig {
    pub fn local(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            target: RuntimeTarget::Local(LocalRuntimeConfig::new(data_dir)),
        }
    }

    pub fn from_env() -> Result<Self, LibVmError> {
        Ok(Self::local(resolve_default_data_dir()?))
    }

    pub fn with_networking(mut self, networking: RuntimeNetworkingConfig) -> Self {
        match &mut self.target {
            RuntimeTarget::Local(local) => local.networking = networking,
        }
        self
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RuntimeTarget {
    Local(LocalRuntimeConfig),
}

#[derive(Debug, Clone)]
pub struct LocalRuntimeConfig {
    pub data_dir: PathBuf,
    pub networking: RuntimeNetworkingConfig,
}

impl LocalRuntimeConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            networking: RuntimeNetworkingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeNetworkingConfig {
    pub private_driver: NetworkDriverKind,
    pub policy_config_dir: Option<PathBuf>,
    pub netd: NetdRuntimeConfig,
}

impl Default for RuntimeNetworkingConfig {
    fn default() -> Self {
        Self {
            private_driver: NetworkDriverKind::Netd,
            policy_config_dir: None,
            netd: NetdRuntimeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetdRuntimeConfig {
    pub subnet: String,
    pub pcap: bool,
    pub tls_ca_cert: Option<PathBuf>,
    pub tls_ca_key: Option<PathBuf>,
}

impl Default for NetdRuntimeConfig {
    fn default() -> Self {
        Self {
            subnet: "192.168.105.0/24".to_string(),
            pcap: false,
            tls_ca_cert: None,
            tls_ca_key: None,
        }
    }
}
