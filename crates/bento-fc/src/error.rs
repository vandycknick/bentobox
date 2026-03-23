use thiserror::Error;

#[derive(Debug, Error)]
pub enum FirecrackerError {
    #[error("failed to build Firecracker HTTP client: {0}")]
    HttpClient(#[source] reqwest::Error),

    #[error("Firecracker API request failed: {0}")]
    Api(#[from] progenitor_client::Error),

    #[error("missing firecracker builder configuration: {0}")]
    MissingConfiguration(&'static str),

    #[error("virtual machine was started without a vsock device configured")]
    VsockNotConfigured,

    #[error("invalid vsock handshake: {0}")]
    InvalidVsockHandshake(String),

    #[error("failed to spawn Firecracker process: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("timed out waiting for Firecracker API socket at {0}")]
    SocketTimeout(std::path::PathBuf),

    #[error("Firecracker process exited before its API socket became ready")]
    ProcessExited,

    #[error("firecracker child {0} pipe was not available")]
    MissingChildPipe(&'static str),

    #[error("failed to acquire child process handle")]
    ChildHandlePoisoned,

    #[error("failed to signal Firecracker process: {0}")]
    Signal(#[source] nix::errno::Errno),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl FirecrackerError {
    pub(crate) fn api<E>(error: progenitor_client::Error<E>) -> Self
    where
        E: std::fmt::Debug,
    {
        Self::Api(error.into_untyped())
    }
}
