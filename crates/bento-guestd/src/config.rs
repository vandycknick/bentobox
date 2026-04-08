use bento_core::services::GuestRuntimeConfig;

const DEFAULT_CONFIG_PATH: &str = "/etc/bento/guestd.yaml";

pub type GuestdConfig = GuestRuntimeConfig;

pub fn load_guestd_config() -> eyre::Result<GuestdConfig> {
    let path = std::path::Path::new(DEFAULT_CONFIG_PATH);
    let config = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        serde_yaml_ng::from_str(&raw)?
    } else {
        GuestdConfig::default()
    };

    Ok(config)
}
