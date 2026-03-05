use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bento_protocol::instance::v1::instance_control_service_client::InstanceControlServiceClient;
use bento_protocol::instance::v1::{
    HealthRequest, LifecycleState, StatusSource, WatchStatusRequest,
};
use eyre::bail;
use hyper_util::rt::TokioIo;
use tokio::sync::Mutex;
use tonic::transport::Endpoint;
use tower::service_fn;

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

pub async fn wait_for_guest_running(socket_path: &Path, timeout: Duration) -> eyre::Result<()> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|err| eyre::eyre!("connect Negotiate socket failed: {err}"))?;

    let stream =
        Negotiate::client_upgrade_stream_v1(stream, Upgrade::InstanceControl { api_version: 1 })
            .await
            .map_err(|err| match err {
                ClientUpgradeStreamError::Io(io_err) => {
                    eyre::eyre!("negotiate instance_control stream failed: {io_err}")
                }
                ClientUpgradeStreamError::Reject(reject) => {
                    eyre::eyre!("{}: {}", reject_code_label(reject.code), reject.message)
                }
            })?;

    let mut client = instance_control_client(stream)
        .await
        .map_err(|err| eyre::eyre!("connect instance control rpc client: {err}"))?;

    let mut updates = client
        .watch_status(WatchStatusRequest {})
        .await
        .map_err(|err| eyre::eyre!("instance control watch_status rpc failed: {err}"))?
        .into_inner();

    let deadline = Instant::now() + timeout;
    let mut vm_running_seen = false;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(eyre::eyre!(
                "timed out after {:?} waiting for guest running event",
                timeout
            ));
        }

        let update = tokio::time::timeout(remaining, updates.message())
            .await
            .map_err(|_| eyre::eyre!("timed out waiting for status updates"))?
            .map_err(|err| eyre::eyre!("watch_status stream failed: {err}"))?;

        let Some(update) = update else {
            return Err(eyre::eyre!(
                "watch_status stream closed before guest became ready"
            ));
        };

        let source = StatusSource::try_from(update.source).unwrap_or(StatusSource::Unspecified);
        let state = LifecycleState::try_from(update.state).unwrap_or(LifecycleState::Unspecified);

        if source == StatusSource::Vm {
            match state {
                LifecycleState::Running => vm_running_seen = true,
                LifecycleState::Stopped | LifecycleState::Error => {
                    return Err(eyre::eyre!(
                        "vm entered {:?} before guest running event",
                        state
                    ));
                }
                _ => {}
            }
        }

        if source == StatusSource::Guest && state == LifecycleState::Running {
            if vm_running_seen {
                return Ok(());
            }

            return Err(eyre::eyre!(
                "received guest running event before vm running event"
            ));
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
        Ok(stream) => {
            let health = call_instance_control_health(stream).await?;
            if health.ok {
                Ok(())
            } else {
                let message = if health.message.is_empty() {
                    "instance control health check failed".to_string()
                } else {
                    health.message
                };
                Err(ProbeError::Retryable(message))
            }
        }
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

async fn call_instance_control_health(
    stream: tokio::net::UnixStream,
) -> Result<bento_protocol::instance::v1::HealthResponse, ProbeError> {
    let mut client = instance_control_client(stream).await.map_err(|err| {
        ProbeError::Retryable(format!("connect instance control rpc client: {err}"))
    })?;

    let response = client.health(HealthRequest {}).await.map_err(|err| {
        ProbeError::Retryable(format!("instance control health rpc failed: {err}"))
    })?;

    Ok(response.into_inner())
}

async fn instance_control_client(
    stream: tokio::net::UnixStream,
) -> Result<InstanceControlServiceClient<tonic::transport::Channel>, tonic::transport::Error> {
    let stream_slot = Arc::new(Mutex::new(Some(stream)));
    let connector = service_fn(move |_| {
        let stream_slot = Arc::clone(&stream_slot);
        async move {
            let mut guard = stream_slot.lock().await;
            guard
                .take()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "instance control connector stream already consumed",
                    )
                })
                .map(TokioIo::new)
        }
    });

    let channel = Endpoint::from_static("http://instance-control.local")
        .connect_with_connector(connector)
        .await?;

    Ok(InstanceControlServiceClient::new(channel))
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
