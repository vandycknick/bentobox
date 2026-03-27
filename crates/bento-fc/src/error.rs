use thiserror::Error;

pub(crate) trait ApiErrorBody: std::fmt::Debug {
    fn fault_message(&self) -> Option<&str> {
        None
    }
}

impl ApiErrorBody for () {}

impl ApiErrorBody for crate::types::Error {
    fn fault_message(&self) -> Option<&str> {
        self.fault_message.as_deref()
    }
}

#[derive(Debug, Error)]
pub enum FirecrackerError {
    #[error("failed to build Firecracker HTTP client: {0}")]
    HttpClient(#[source] reqwest::Error),

    #[error(
        "Firecracker API request failed during {operation}: {detail}{fault_suffix}",
        fault_suffix = api_fault_suffix(fault_message.as_deref())
    )]
    Api {
        operation: &'static str,
        status: Option<reqwest::StatusCode>,
        fault_message: Option<String>,
        detail: String,
    },

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
    pub(crate) fn api<E>(operation: &'static str, error: progenitor_client::Error<E>) -> Self
    where
        E: ApiErrorBody,
    {
        let status = error.status();
        let fault_message = match &error {
            progenitor_client::Error::ErrorResponse(response) => {
                response.fault_message().map(ToOwned::to_owned)
            }
            _ => None,
        };
        let detail = error.to_string();

        Self::Api {
            operation,
            status,
            fault_message,
            detail,
        }
    }

    pub fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            Self::Api { status, .. } => *status,
            _ => None,
        }
    }

    pub fn fault_message(&self) -> Option<&str> {
        match self {
            Self::Api { fault_message, .. } => fault_message.as_deref(),
            _ => None,
        }
    }

    pub fn operation(&self) -> Option<&'static str> {
        match self {
            Self::Api { operation, .. } => Some(*operation),
            _ => None,
        }
    }

    pub fn is_send_ctrl_alt_del_unsupported_on_aarch64(&self) -> bool {
        matches!(
            self,
            Self::Api {
                operation: "create_sync_action",
                fault_message: Some(fault_message),
                ..
            } if fault_message == "SendCtrlAltDel does not supported on aarch64."
        )
    }
}

fn api_fault_suffix(fault_message: Option<&str>) -> String {
    match fault_message {
        Some(fault_message) => format!("; fault_message: {fault_message}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::FirecrackerError;
    use crate::types;
    use progenitor_client::{Error, ResponseValue};
    use reqwest::StatusCode;

    #[test]
    fn preserves_fault_message_when_formatting_api_errors() {
        let response = ResponseValue::new(
            types::Error {
                fault_message: Some(
                    "SendCtrlAltDel is not supported on this architecture".to_string(),
                ),
            },
            StatusCode::BAD_REQUEST,
            Default::default(),
        );

        let error = FirecrackerError::api("create_sync_action", Error::ErrorResponse(response));

        assert_eq!(error.status(), Some(StatusCode::BAD_REQUEST));
        assert_eq!(error.operation(), Some("create_sync_action"));
        assert!(error.to_string().contains("fault_message"));
        assert!(error
            .to_string()
            .contains("SendCtrlAltDel is not supported"));
    }

    #[test]
    fn identifies_aarch64_ctrl_alt_del_rejection() {
        let response = ResponseValue::new(
            types::Error {
                fault_message: Some("SendCtrlAltDel does not supported on aarch64.".to_string()),
            },
            StatusCode::BAD_REQUEST,
            Default::default(),
        );

        let error = FirecrackerError::api("create_sync_action", Error::ErrorResponse(response));

        assert!(error.is_send_ctrl_alt_del_unsupported_on_aarch64());
    }
}
