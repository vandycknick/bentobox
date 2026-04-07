use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LibVmError {
    #[error("could not resolve Bento data directory from XDG_DATA_HOME or HOME")]
    DataDirUnavailable,

    #[error("environment variable {name} must be an absolute path, got {path}")]
    RelativeEnvironmentPath { name: &'static str, path: PathBuf },

    #[error("invalid machine name {name:?}: {reason}")]
    InvalidMachineName { name: String, reason: String },
}
