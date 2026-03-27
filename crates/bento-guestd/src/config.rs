use bento_runtime::capabilities::CapabilitiesConfig;
use serde::Deserialize;

const DEFAULT_CONFIG_PATH: &str = "/etc/bento/guestd.yaml";

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GuestdConfig {
    #[serde(default)]
    pub capabilities: CapabilitiesConfig,
}

impl GuestdConfig {
    pub fn load() -> eyre::Result<Self> {
        let path = std::path::Path::new(DEFAULT_CONFIG_PATH);
        let config = if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            serde_yaml_ng::from_str(&raw)?
        } else {
            Self::default()
        };

        Ok(config)
    }
}
