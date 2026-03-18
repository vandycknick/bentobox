use serde::{Deserialize, Serialize};

use crate::extensions::ExtensionsConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageMetadata {
    pub schema_version: u32,
    pub os: String,
    pub arch: String,
    pub defaults: ImageMetadataDefaults,
    #[serde(default)]
    pub bootstrap: ImageMetadataBootstrap,
    #[serde(default)]
    pub extensions: ExtensionsConfig,
}

impl Default for ImageMetadata {
    fn default() -> Self {
        Self {
            schema_version: 1,
            os: "linux".to_string(),
            arch: host_arch().to_string(),
            defaults: ImageMetadataDefaults::default(),
            bootstrap: ImageMetadataBootstrap::default(),
            extensions: ExtensionsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ImageMetadataBootstrap {
    pub cidata_cloud_init: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageMetadataDefaults {
    pub cpu: u8,
    pub memory_mib: u32,
}

impl Default for ImageMetadataDefaults {
    fn default() -> Self {
        Self {
            cpu: 1,
            memory_mib: 512,
        }
    }
}

pub fn host_arch() -> &'static str {
    // Rust and target triples use `aarch64`, while OCI/image tooling conventionally uses
    // `arm64`. Normalize here so Bentobox metadata matches the OCI-facing arch name.
    // TODO: Revisit whether image metadata should keep stringly OCI arch names or move to a
    // stronger typed platform model with explicit conversions at the boundaries.
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}
