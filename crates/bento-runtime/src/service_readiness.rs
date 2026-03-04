use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use eyre::bail;

use crate::negotiate::{ClientUpgradeStreamError, Negotiate, RejectCode, Upgrade};
use crate::services::{ServiceDescriptor, SERVICE_SERIAL, SERVICE_SSH};

pub const DEFAULT_SERVICE_READINESS_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const DEFAULT_SERVICE_READINESS_POLL_INTERVAL: Duration = Duration::from_secs(1);

enum ProbeError {
    Retryable(String),
    Fatal(String),
}

pub async fn wait_for_services(socket_path: &Path) -> eyre::Result<Vec<ServiceDescriptor>> {
    wait_for_services_with_timeout(
        socket_path,
        DEFAULT_SERVICE_READINESS_TIMEOUT,
        DEFAULT_SERVICE_READINESS_POLL_INTERVAL,
    )
    .await
}

pub async fn wait_for_services_with_timeout(
    socket_path: &Path,
    timeout: Duration,
    poll_interval: Duration,
) -> eyre::Result<Vec<ServiceDescriptor>> {
    let deadline = Instant::now() + timeout;

    loop {
        match probe_instance_control_once(socket_path).await {
            Ok(()) => {
                return Ok(vec![
                    ServiceDescriptor {
                        name: SERVICE_SERIAL.to_string(),
                    },
                    ServiceDescriptor {
                        name: SERVICE_SSH.to_string(),
                    },
                ])
            }
            Err(ProbeError::Retryable(message)) => {
                if Instant::now() >= deadline {
                    bail!(
                        "timed out waiting {:?} for guest service readiness via instance control (last error: {})",
                        timeout,
                        message
                    );
                }

                tokio::time::sleep(poll_interval).await;
            }
            Err(ProbeError::Fatal(message)) => {
                bail!("{message}");
            }
        }
    }
}

async fn probe_instance_control_once(socket_path: &Path) -> Result<(), ProbeError> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|err| classify_io_error("connect Negotiate socket", err))?;

    match Negotiate::client_upgrade_stream_v1(stream, Upgrade::InstanceControl { api_version: 1 })
        .await
    {
        Ok(_stream) => Ok(()),
        Err(ClientUpgradeStreamError::Io(err)) => {
            Err(classify_io_error("negotiate instance_control stream", err))
        }
        Err(ClientUpgradeStreamError::Reject(reject)) => match reject.code {
            RejectCode::ServiceStarting | RejectCode::ServiceUnavailable => {
                Err(ProbeError::Retryable(format!(
                    "{}: {}",
                    reject_code_label(reject.code),
                    reject.message
                )))
            }
            _ => Err(ProbeError::Fatal(format!(
                "{}: {}",
                reject_code_label(reject.code),
                reject.message
            ))),
        },
    }
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

fn reject_code_label(code: RejectCode) -> &'static str {
    match code {
        RejectCode::UnsupportedProtocol => "unsupported_protocol",
        RejectCode::UnsupportedUpgrade => "unsupported_upgrade",
        RejectCode::UnsupportedService => "unsupported_service",
        RejectCode::ServiceStarting => "service_starting",
        RejectCode::ServiceUnavailable => "service_unavailable",
        RejectCode::PermissionDenied => "permission_denied",
        RejectCode::AuthFailed => "auth_failed",
        RejectCode::Internal => "internal_error",
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
