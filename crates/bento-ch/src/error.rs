use thiserror::Error;

#[derive(Debug, Error)]
pub enum CloudHypervisorError {
    #[error("failed to build Cloud Hypervisor HTTP client: {0}")]
    HttpClient(#[source] reqwest::Error),

    #[error("Cloud Hypervisor API request failed during {operation}: {detail}")]
    Api {
        operation: &'static str,
        status: Option<reqwest::StatusCode>,
        detail: String,
    },

    #[error("missing Cloud Hypervisor builder configuration: {0}")]
    MissingConfiguration(&'static str),

    #[error("failed to spawn Cloud Hypervisor process: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("timed out waiting for Cloud Hypervisor API socket at {0}")]
    ApiTimeout(std::path::PathBuf),

    #[error("Cloud Hypervisor process exited before its API became ready")]
    ProcessExited,

    #[error("virtual machine was started without a vsock device configured")]
    VsockNotConfigured,

    #[error("invalid vsock handshake: {0}")]
    InvalidVsockHandshake(String),

    #[error("failed to signal Cloud Hypervisor process: {0}")]
    Signal(#[source] nix::errno::Errno),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl CloudHypervisorError {
    pub(crate) fn api<E>(operation: &'static str, error: progenitor_client::Error<E>) -> Self
    where
        E: std::fmt::Debug,
    {
        Self::Api {
            operation,
            status: error.status(),
            detail: error.to_string(),
        }
    }
}
