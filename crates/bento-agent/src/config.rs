use bento_core::services::GuestRuntimeConfig;

const DEFAULT_CONFIG_PATH: &str = "/etc/bento/agent.yaml";

pub type AgentConfig = GuestRuntimeConfig;

pub fn load_agent_config() -> eyre::Result<AgentConfig> {
    let path = std::path::Path::new(DEFAULT_CONFIG_PATH);
    let config = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        serde_yaml_ng::from_str(&raw)?
    } else {
        AgentConfig::default()
    };

    Ok(config)
}
