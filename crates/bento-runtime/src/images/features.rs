use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::extensions::ExtensionsConfig;

pub const BOOTSTRAP_CIDATA_CLOUD_INIT: &str = "sh.nvd.bento.bootstrap.cidata_cloud_init";
pub const EXT_SSH: &str = "sh.nvd.bento.ext.ssh";
pub const EXT_DOCKER: &str = "sh.nvd.bento.ext.docker";
pub const EXT_PORT_FORWARD: &str = "sh.nvd.bento.ext.port_forward";

const LEGACY_CAP_CLOUD_INIT: &str = "sh.nvd.bento.cap.cloud_init";
const LEGACY_CAP_SSH: &str = "sh.nvd.bento.cap.ssh";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ImageFeatures {
    #[serde(default)]
    pub bootstrap_cidata_cloud_init: bool,
    #[serde(default)]
    pub extensions: ExtensionsConfig,
}

impl ImageFeatures {
    pub fn supports_bootstrap(&self) -> bool {
        self.bootstrap_cidata_cloud_init
    }

    pub fn from_annotations(annotations: &BTreeMap<String, String>) -> Self {
        Self {
            bootstrap_cidata_cloud_init: parse_enabled(
                annotations.get(BOOTSTRAP_CIDATA_CLOUD_INIT),
            ) || parse_enabled(annotations.get(LEGACY_CAP_CLOUD_INIT)),
            extensions: ExtensionsConfig {
                ssh: parse_enabled(annotations.get(EXT_SSH))
                    || parse_enabled(annotations.get(LEGACY_CAP_SSH)),
                docker: parse_enabled(annotations.get(EXT_DOCKER)),
                port_forward: parse_enabled(annotations.get(EXT_PORT_FORWARD)),
            },
        }
    }
}

fn parse_enabled(value: Option<&String>) -> bool {
    value.map(|raw| raw.as_str()) == Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_annotations_supports_bootstrap_extensions_and_legacy_keys() {
        let mut annotations = BTreeMap::new();
        annotations.insert(LEGACY_CAP_CLOUD_INIT.to_string(), "true".to_string());
        annotations.insert(EXT_DOCKER.to_string(), "true".to_string());
        annotations.insert(EXT_PORT_FORWARD.to_string(), "true".to_string());
        annotations.insert(LEGACY_CAP_SSH.to_string(), "true".to_string());

        let features = ImageFeatures::from_annotations(&annotations);
        assert!(features.supports_bootstrap());
        assert!(features.extensions.ssh);
        assert!(features.extensions.docker);
        assert!(features.extensions.port_forward);
    }
}
