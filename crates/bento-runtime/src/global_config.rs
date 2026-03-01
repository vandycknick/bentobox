use std::path::{Path, PathBuf};

use eyre::Context;
use serde::Deserialize;

use crate::directories::Directory;

const CONFIG_FILE_NAME: &str = "config.yaml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalConfig {
    pub guest_agent_binary: PathBuf,
}

impl GlobalConfig {
    pub fn load() -> eyre::Result<Self> {
        let config_path = config_path()?;
        let raw = std::fs::read_to_string(&config_path)
            .with_context(|| format!("read global config {}", config_path.display()))?;

        parse_global_config(&raw).with_context(|| {
            format!(
                "parse global config {} (expected guest.agent_binary in yaml)",
                config_path.display()
            )
        })
    }
}

fn config_path() -> eyre::Result<PathBuf> {
    Directory::with_prefix("")
        .get_config_home()
        .map(|base| base.join(CONFIG_FILE_NAME))
        .ok_or_else(|| eyre::eyre!("resolve ~/.config/bento path"))
}

fn parse_global_config(input: &str) -> eyre::Result<GlobalConfig> {
    let parsed: RawGlobalConfig =
        serde_yaml_ng::from_str(input).context("deserialize global config yaml")?;

    let guest_agent_binary = parsed.guest.agent_binary;

    if !guest_agent_binary.is_absolute() {
        return Err(eyre::eyre!(
            "[guest].agent_binary must be an absolute path: {}",
            guest_agent_binary.display()
        ));
    }

    Ok(GlobalConfig { guest_agent_binary })
}

#[derive(Debug, Deserialize)]
struct RawGlobalConfig {
    guest: RawGuestConfig,
}

#[derive(Debug, Deserialize)]
struct RawGuestConfig {
    agent_binary: PathBuf,
}

pub fn ensure_guest_agent_binary(config: &GlobalConfig) -> eyre::Result<&Path> {
    let path = config.guest_agent_binary.as_path();
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("stat guest agent binary {}", path.display()))?;

    if !metadata.is_file() {
        return Err(eyre::eyre!(
            "guest agent path is not a file: {}",
            path.display()
        ));
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_global_config_reads_guest_agent_binary() {
        let cfg = parse_global_config(
            r#"
guest:
  agent_binary: "/tmp/bento-instance-guest"
"#,
        )
        .expect("parse config");

        assert_eq!(
            cfg.guest_agent_binary,
            PathBuf::from("/tmp/bento-instance-guest")
        );
    }

    #[test]
    fn parse_global_config_rejects_missing_guest_key() {
        let result = parse_global_config(
            r#"
guest:
  other: "value"
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn parse_global_config_rejects_relative_paths() {
        let err = parse_global_config(
            r#"
guest:
  agent_binary: "./bento-instance-guest"
"#,
        )
        .expect_err("relative path should fail");

        assert!(err.to_string().contains("absolute path"));
    }
}
