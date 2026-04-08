use std::path::Path;

use bento_core::{InstanceFile, VmSpec};
use eyre::Context;

pub fn read_vm_spec_from_dir(data_dir: &Path) -> eyre::Result<VmSpec> {
    let config_path = data_dir.join(InstanceFile::Config.as_str());
    let raw = std::fs::read_to_string(&config_path)
        .wrap_err_with(|| format!("read vm spec at {}", config_path.display()))?;
    serde_yaml_ng::from_str(&raw)
        .map_err(|err| eyre::eyre!("parse vm spec at {}: {}", config_path.display(), err))
}

pub fn format_error_chain(err: &eyre::Report) -> String {
    let mut parts = Vec::new();
    for cause in err.chain() {
        parts.push(cause.to_string());
    }
    parts.join(": ")
}

pub fn remove_stale_socket(path: &Path) -> eyre::Result<()> {
    if let Err(err) = std::fs::remove_file(path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err).context(format!("remove stale socket {}", path.display()));
        }
    }

    Ok(())
}
