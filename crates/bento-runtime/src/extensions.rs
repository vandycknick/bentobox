use serde::{Deserialize, Serialize};

pub const EXTENSION_SSH: &str = "ssh";
pub const EXTENSION_DOCKER: &str = "docker";
pub const EXTENSION_PORT_FORWARD: &str = "port-forward";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinExtension {
    Ssh,
    Docker,
    PortForward,
}

impl BuiltinExtension {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Ssh => EXTENSION_SSH,
            Self::Docker => EXTENSION_DOCKER,
            Self::PortForward => EXTENSION_PORT_FORWARD,
        }
    }

    pub fn startup_required(&self) -> bool {
        matches!(self, Self::Ssh)
    }

    pub fn requires_bootstrap(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ExtensionsConfig {
    #[serde(default)]
    pub ssh: bool,
    #[serde(default)]
    pub docker: bool,
    #[serde(default)]
    pub port_forward: bool,
}

impl ExtensionsConfig {
    pub fn is_empty(&self) -> bool {
        !self.ssh && !self.docker && !self.port_forward
    }

    pub fn is_enabled(&self, extension: BuiltinExtension) -> bool {
        match extension {
            BuiltinExtension::Ssh => self.ssh,
            BuiltinExtension::Docker => self.docker,
            BuiltinExtension::PortForward => self.port_forward,
        }
    }

    pub fn enabled_extensions(&self) -> Vec<BuiltinExtension> {
        let mut extensions = Vec::new();
        if self.ssh {
            extensions.push(BuiltinExtension::Ssh);
        }
        if self.docker {
            extensions.push(BuiltinExtension::Docker);
        }
        if self.port_forward {
            extensions.push(BuiltinExtension::PortForward);
        }
        extensions
    }

    pub fn requires_bootstrap(&self) -> bool {
        self.enabled_extensions()
            .into_iter()
            .any(|extension| extension.requires_bootstrap())
    }
}
