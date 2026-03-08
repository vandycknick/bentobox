use bento_runtime::extensions::ExtensionsConfig;
use serde::Deserialize;

const DEFAULT_CONFIG_PATH: &str = "/etc/bento/guestd.yaml";

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GuestdConfig {
    #[serde(default)]
    pub extensions: ExtensionsConfig,
}

impl GuestdConfig {
    pub fn load() -> eyre::Result<Self> {
        let path = std::path::Path::new(DEFAULT_CONFIG_PATH);
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(path)?;
        Ok(serde_yaml_ng::from_str(&raw)?)
    }
}
