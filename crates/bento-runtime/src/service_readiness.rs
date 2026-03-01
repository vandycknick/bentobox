use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use eyre::bail;

use crate::instance_control::{
    ControlErrorCode, ControlRequest, ControlResponse, ControlResponseBody, ServiceDescriptor,
    CONTROL_PROTOCOL_VERSION,
};

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
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|err| classify_io_error("connect control socket", err))?;

    stream
        .set_read_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(|err| classify_io_error("set control socket read timeout", err))?;
    stream
        .set_write_timeout(Some(CONTROL_IO_TIMEOUT))
        .map_err(|err| classify_io_error("set control socket write timeout", err))?;

    ControlRequest::v1_list_services("guest-service-readiness")
        .write_to(&mut stream)
        .map_err(|err| classify_io_error("write list_services request", err))?;

    let response = ControlResponse::read_from(&mut stream)
        .map_err(|err| classify_io_error("read list_services response", err))?;

    if response.version != CONTROL_PROTOCOL_VERSION {
        return Err(ProbeError::Fatal(format!(
            "unsupported_version: daemon returned protocol version {}, expected {}",
            response.version, CONTROL_PROTOCOL_VERSION
        )));
    }

    match response.body {
        ControlResponseBody::Services { services } => Ok(services),
        ControlResponseBody::Error { code, message } => match code {
            ControlErrorCode::ServiceUnavailable | ControlErrorCode::InstanceNotRunning => Err(
                ProbeError::Retryable(format!("{}: {message}", error_code_label(&code))),
            ),
            _ => Err(ProbeError::Fatal(format!(
                "{}: {message}",
                error_code_label(&code)
            ))),
        },
        ControlResponseBody::Opened => Err(ProbeError::Fatal(
            "invalid_response: expected services response for list_services request".to_string(),
        )),
        ControlResponseBody::Starting { .. } => Err(ProbeError::Retryable(
            "service_starting: daemon is still preparing guest services".to_string(),
        )),
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
