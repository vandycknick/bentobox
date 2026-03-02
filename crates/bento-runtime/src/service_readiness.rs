use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use bento_protocol::control::{ControlErrorCode, ControlPlaneClient, ServiceDescriptor};
use eyre::bail;

pub const DEFAULT_SERVICE_READINESS_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const DEFAULT_SERVICE_READINESS_POLL_INTERVAL: Duration = Duration::from_secs(1);
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(5);

enum ProbeError {
    Retryable(String),
    Fatal(String),
}

pub fn wait_for_services(socket_path: &Path) -> eyre::Result<Vec<ServiceDescriptor>> {
    wait_for_services_with_timeout(
        socket_path,
        DEFAULT_SERVICE_READINESS_TIMEOUT,
        DEFAULT_SERVICE_READINESS_POLL_INTERVAL,
    )
}

pub fn wait_for_services_with_timeout(
    socket_path: &Path,
    timeout: Duration,
    poll_interval: Duration,
) -> eyre::Result<Vec<ServiceDescriptor>> {
    let deadline = Instant::now() + timeout;

    loop {
        match list_services_once(socket_path) {
            Ok(services) => return Ok(services),
            Err(ProbeError::Retryable(message)) => {
                if Instant::now() >= deadline {
                    bail!(
                        "timed out waiting {:?} for guest service discovery readiness (last error: {})",
                        timeout,
                        message
                    );
                }

                thread::sleep(poll_interval);
            }
            Err(ProbeError::Fatal(message)) => {
                bail!("{message}");
            }
        }
    }
}

fn list_services_once(socket_path: &Path) -> Result<Vec<ServiceDescriptor>, ProbeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| ProbeError::Fatal(format!("build tokio runtime failed: {err}")))?;

    runtime.block_on(async move {
        use tarpc::context;
        use tarpc::serde_transport;
        use tarpc::tokio_serde::formats::Bincode;
        use tarpc::tokio_util::codec::LengthDelimitedCodec;

        let stream = tokio::time::timeout(
            CONTROL_IO_TIMEOUT,
            tokio::net::UnixStream::connect(socket_path),
        )
        .await
        .map_err(|_| ProbeError::Retryable("connect control socket timed out".to_string()))
        .and_then(|result| {
            result.map_err(|err| classify_io_error("connect control socket", err))
        })?;

        let framed = LengthDelimitedCodec::builder().new_framed(stream);
        let transport = serde_transport::new(framed, Bincode::default());
        let client = ControlPlaneClient::new(tarpc::client::Config::default(), transport).spawn();

        let rpc_result =
            tokio::time::timeout(CONTROL_IO_TIMEOUT, client.list_services(context::current()))
                .await
                .map_err(|_| {
                    ProbeError::Retryable("list_services request timed out".to_string())
                })?;

        let service_result = rpc_result.map_err(|err| {
            ProbeError::Retryable(format!("list_services transport failed: {err}"))
        })?;

        service_result.map_err(|err| match err.code {
            ControlErrorCode::ServiceUnavailable | ControlErrorCode::InstanceNotRunning => {
                ProbeError::Retryable(format!("{}: {}", error_code_label(&err.code), err.message))
            }
            _ => ProbeError::Fatal(format!("{}: {}", error_code_label(&err.code), err.message)),
        })
    })
}

fn classify_io_error(context: &str, err: io::Error) -> ProbeError {
    if is_retryable_io_kind(err.kind()) {
        return ProbeError::Retryable(format!("{context} failed: {err}"));
    }

    ProbeError::Fatal(format!("{context} failed: {err}"))
}

fn is_retryable_io_kind(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::Interrupted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
}

fn error_code_label(code: &ControlErrorCode) -> &'static str {
    match code {
        ControlErrorCode::UnsupportedVersion => "unsupported_version",
        ControlErrorCode::UnsupportedRequest => "unsupported_request",
        ControlErrorCode::UnknownService => "unknown_service",
        ControlErrorCode::ServiceUnavailable => "service_unavailable",
        ControlErrorCode::InstanceNotRunning => "instance_not_running",
        ControlErrorCode::PermissionDenied => "permission_denied",
        ControlErrorCode::Internal => "internal_error",
    }
}

#[cfg(test)]
mod tests {
    use super::is_retryable_io_kind;

    #[test]
    fn retryable_io_kinds_cover_startup_transients() {
        assert!(is_retryable_io_kind(std::io::ErrorKind::NotFound));
        assert!(is_retryable_io_kind(std::io::ErrorKind::ConnectionRefused));
        assert!(is_retryable_io_kind(std::io::ErrorKind::TimedOut));
        assert!(is_retryable_io_kind(std::io::ErrorKind::UnexpectedEof));
        assert!(!is_retryable_io_kind(std::io::ErrorKind::InvalidData));
        assert!(!is_retryable_io_kind(std::io::ErrorKind::PermissionDenied));
    }
}
