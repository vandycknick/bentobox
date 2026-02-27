use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const CAP_CLOUD_INIT: &str = "sh.nvd.bento.cap.cloud_init";
pub const CAP_SSH: &str = "sh.nvd.bento.cap.ssh";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    CloudInit,
    Ssh,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GuestCapabilities {
    #[serde(default)]
    pub cloud_init: bool,
    #[serde(default)]
    pub ssh: bool,
}

impl GuestCapabilities {
    pub fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::CloudInit => self.cloud_init,
            Capability::Ssh => self.ssh,
        }
    }

    pub fn supports_any(&self, capabilities: &[Capability]) -> bool {
        capabilities
            .iter()
            .any(|capability| self.supports(*capability))
    }

    pub fn supports_all(&self, capabilities: &[Capability]) -> bool {
        capabilities
            .iter()
            .all(|capability| self.supports(*capability))
    }

    pub fn is_empty(&self) -> bool {
        !self.cloud_init && !self.ssh
    }

    pub fn from_annotations(annotations: &BTreeMap<String, String>) -> Self {
        Self {
            cloud_init: parse_capability_true(annotations.get(CAP_CLOUD_INIT)),
            ssh: parse_capability_true(annotations.get(CAP_SSH)),
        }
    }
}

fn parse_capability_true(value: Option<&String>) -> bool {
    value.map(|raw| raw.as_str()) == Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_annotations_only_treats_literal_true_as_enabled() {
        let mut annotations = BTreeMap::new();
        annotations.insert(CAP_CLOUD_INIT.to_string(), "true".to_string());
        annotations.insert(CAP_SSH.to_string(), "false".to_string());

        let caps = GuestCapabilities::from_annotations(&annotations);
        assert!(caps.cloud_init);
        assert!(!caps.ssh);
    }

    #[test]
    fn supports_helpers_apply_expected_set_logic() {
        let caps = GuestCapabilities {
            cloud_init: true,
            ssh: false,
        };

        assert!(caps.supports(Capability::CloudInit));
        assert!(!caps.supports(Capability::Ssh));
        assert!(caps.supports_any(&[Capability::Ssh, Capability::CloudInit]));
        assert!(!caps.supports_all(&[Capability::Ssh, Capability::CloudInit]));
    }
}
