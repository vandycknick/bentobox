use std::fmt::{Display, Formatter};
use std::io;
use std::time::{Duration, Instant};

use bento_instanced::launcher::NixLauncher;
use bento_runtime::instance::{InstanceFile, InstanceStatus};
use bento_runtime::instance_manager::InstanceManager;
use bento_runtime::negotiate::{
    ClientUpgradeStreamError, Negotiate, ProxyMode, RejectCode, Upgrade,
};
use bento_runtime::service_readiness;
use bento_runtime::services::SERVICE_SSH;
use clap::Args;
use eyre::{bail, Context};
use tokio::io::AsyncWriteExt;

#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Cmd {
    #[arg(long)]
    pub name: String,

    #[arg(long, default_value = SERVICE_SSH)]
    pub service: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "--name {} --service {}", self.name, self.service)
    }
}

impl Cmd {
    pub async fn run(&self, manager: &InstanceManager<NixLauncher>) -> eyre::Result<()> {
        let inst = manager.inspect(&self.name)?;
        let socket_path = inst.file(InstanceFile::InstancedSocket);

        let should_wait_for_guest_readiness =
            inst.status() == InstanceStatus::Running && inst.expects_guest_agent();
        let deadline = Instant::now() + service_readiness::DEFAULT_SERVICE_READINESS_TIMEOUT;

        loop {
            match try_open_service_once(&socket_path, &self.service).await {
                Ok(stream) => return proxy_stdio(stream).await,
                Err(ControlClientError::Fatal { message }) => bail!("{message}"),
                Err(ControlClientError::Retryable { message }) => {
                    if !should_wait_for_guest_readiness {
                        bail!("{message}");
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        bail!(
                            "timed out waiting {:?} for guest ssh readiness (last error: {message})",
                            service_readiness::DEFAULT_SERVICE_READINESS_TIMEOUT
                        );
                    }

                    let services = service_readiness::wait_for_services_with_timeout(
                        &socket_path,
                        remaining,
                        Duration::from_secs(1),
                    )
                    .await
                    .context("wait for guest service discovery readiness")?;

                    if self.service == SERVICE_SSH
                        && services.iter().all(|service| service.name != SERVICE_SSH)
                    {
                        let available = if services.is_empty() {
                            "none".to_string()
                        } else {
                            services
                                .iter()
                                .map(|service| service.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        };

                        bail!(
                            "unsupported_service: guest discovery is ready but ssh is not supported (available services: {available})"
                        );
                    }
                }
            }
        }
    }
}

async fn try_open_service_once(
    socket_path: &std::path::Path,
    service: &str,
) -> Result<tokio::net::UnixStream, ControlClientError> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                ControlClientError::Retryable {
                    message: format!(
                        "instanced_unreachable: control socket {} is missing, make sure the VM is running",
                        socket_path.display()
                    ),
                }
            } else {
                classify_io_error(
                    "connect control socket",
                    io::Error::new(err.kind(), format!("{} ({})", err, socket_path.display())),
                )
            }
        })?;

    match Negotiate::client_upgrade_stream_v1(
        stream,
        Upgrade::Proxy {
            service: service.to_string(),
            mode: ProxyMode::ReadWrite,
        },
    )
    .await
    {
        Ok(stream) => Ok(stream),
        Err(ClientUpgradeStreamError::Reject(reject)) => {
            Err(classify_reject_code(reject.code, &reject.message))
        }
        Err(ClientUpgradeStreamError::Io(err)) => {
            Err(classify_io_error("negotiate proxy stream", err))
        }
    }
}

enum ControlClientError {
    Retryable { message: String },
    Fatal { message: String },
}

fn classify_reject_code(code: RejectCode, message: &str) -> ControlClientError {
    match code {
        RejectCode::ServiceStarting | RejectCode::ServiceUnavailable => {
            ControlClientError::Retryable {
                message: render_reject_error(code, message),
            }
        }
        _ => ControlClientError::Fatal {
            message: render_reject_error(code, message),
        },
    }
}

fn classify_io_error(context: &str, err: io::Error) -> ControlClientError {
    let message = format!("{context} failed: {err}");

    if is_retryable_io_kind(err.kind()) {
        return ControlClientError::Retryable { message };
    }

    ControlClientError::Fatal { message }
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

fn render_reject_error(code: RejectCode, message: &str) -> String {
    match code {
        RejectCode::ServiceStarting => {
            format!("service_starting: {message}")
        }
        RejectCode::ServiceUnavailable => {
            format!("service_unavailable: {message}. ensure guest service is running")
        }
        RejectCode::UnsupportedService => {
            format!("unknown_service: {message}. try a supported service like 'ssh'")
        }
        RejectCode::UnsupportedProtocol => {
            format!(
                "unsupported_protocol: {message}. update bentoctl/instanced to matching versions"
            )
        }
        RejectCode::UnsupportedUpgrade => {
            format!("unsupported_upgrade: {message}")
        }
        RejectCode::PermissionDenied => {
            format!("permission_denied: {message}")
        }
        RejectCode::AuthFailed => {
            format!("auth_failed: {message}")
        }
        RejectCode::Internal => {
            format!("internal_error: {message}")
        }
    }
}

async fn proxy_stdio(stream: tokio::net::UnixStream) -> eyre::Result<()> {
    let (mut stream_read, mut stream_write) = stream.into_split();

    let input = async {
        let mut stdin = tokio::io::stdin();
        tokio::io::copy(&mut stdin, &mut stream_write)
            .await
            .context("relay shell input")?;
        stream_write
            .shutdown()
            .await
            .context("shutdown shell input stream")?;
        Ok::<(), eyre::Report>(())
    };

    let output = async {
        let mut stdout = tokio::io::stdout();
        tokio::io::copy(&mut stream_read, &mut stdout)
            .await
            .context("relay shell output")?;
        stdout.flush().await.context("flush shell output")?;
        Ok::<(), eyre::Report>(())
    };

    tokio::try_join!(output, input)?;
    Ok(())
}
