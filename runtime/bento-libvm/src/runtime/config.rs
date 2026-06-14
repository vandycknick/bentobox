use std::path::PathBuf;

use crate::network::NetworkDriverKind;
use crate::paths::resolve_default_data_dir;
use crate::LibVmError;

/// Runtime connection configuration.
///
/// Use `local`, `remote`, or `from_env` to choose the runtime backend.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub(crate) target: RuntimeTarget,
}

impl RuntimeConfig {
    /// Creates a local runtime configuration rooted at `data_dir`.
    pub fn local(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            target: RuntimeTarget::Local(LocalRuntimeConfig::new(data_dir)),
        }
    }

    /// Creates a remote runtime configuration for `endpoint`.
    pub fn remote(endpoint: impl Into<String>) -> Self {
        Self {
            target: RuntimeTarget::Remote(RemoteRuntimeConfig::new(endpoint)),
        }
    }

    /// Creates the default local runtime configuration from the environment.
    pub fn from_env() -> Result<Self, LibVmError> {
        Ok(Self::local(resolve_default_data_dir()?))
    }

    /// Sets local runtime networking configuration.
    ///
    /// Remote runtime configs ignore this because networking is controlled by
    /// the remote service.
    pub fn with_networking(mut self, networking: RuntimeNetworkingConfig) -> Self {
        match &mut self.target {
            RuntimeTarget::Local(local) => local.networking = networking,
            RuntimeTarget::Remote(_) => {}
        }
        self
    }
}

/// Runtime backend target.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RuntimeTarget {
    /// Local runtime using files and SQLite under a local data directory.
    Local(LocalRuntimeConfig),
    /// Remote runtime accessed through an endpoint.
    Remote(RemoteRuntimeConfig),
}

/// Remote runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRuntimeConfig {
    /// Remote endpoint address.
    pub endpoint: String,
}

impl RemoteRuntimeConfig {
    /// Creates a remote runtime configuration.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

/// Local runtime configuration.
#[derive(Debug, Clone)]
pub struct LocalRuntimeConfig {
    /// Data directory used for machine state, images, and runtime files.
    pub data_dir: PathBuf,
    /// Networking configuration for locally started machines.
    pub networking: RuntimeNetworkingConfig,
}

impl LocalRuntimeConfig {
    /// Creates a local runtime configuration with default networking settings.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            networking: RuntimeNetworkingConfig::default(),
        }
    }
}

/// Networking configuration for the local runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeNetworkingConfig {
    /// Driver used for private machine networks.
    pub private_driver: NetworkDriverKind,
    /// Directory containing network policy configuration files.
    pub policy_config_dir: Option<PathBuf>,
    /// netd-specific runtime configuration.
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

/// Configuration for the netd network driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetdRuntimeConfig {
    /// Subnet used for managed private networks.
    pub subnet: String,
    /// Whether packet capture should be enabled.
    pub pcap: bool,
    /// Optional TLS CA certificate path.
    pub tls_ca_cert: Option<PathBuf>,
    /// Optional TLS CA key path.
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
