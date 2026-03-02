use std::fmt::{Display, Formatter};
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use bento_protocol::control::{
    ControlErrorCode, ControlPlaneClient, OpenServiceRequest, SERVICE_SSH,
};
use bento_runtime::instance::{InstanceFile, InstanceStatus};
use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
use bento_runtime::service_readiness;
use clap::Args;
use eyre::{bail, Context};

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
    pub fn run(&self) -> eyre::Result<()> {
        let manager = InstanceManager::new(NixDaemon::new("123"));
        let inst = manager.inspect(&self.name)?;
        let control_socket_path = inst.file(InstanceFile::InstancedSocket);

        let should_wait_for_guest_readiness =
            inst.status() == InstanceStatus::Running && inst.expects_guest_agent();
        let deadline = Instant::now() + service_readiness::DEFAULT_SERVICE_READINESS_TIMEOUT;

        loop {
            match open_service_once(&control_socket_path, &self.service) {
                Ok(stream) => return proxy_stdio(stream),
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
                        &control_socket_path,
                        remaining,
                        Duration::from_secs(1),
                    )
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

fn open_service_once(
    control_socket_path: &std::path::Path,
    service: &str,
) -> Result<UnixStream, ControlClientError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| ControlClientError::Fatal {
            message: format!("build tokio runtime failed: {err}"),
        })?;

    let tunnel_socket_path = runtime.block_on(async move {
        use tarpc::context;
        use tarpc::serde_transport;
        use tarpc::tokio_serde::formats::Bincode;
        use tarpc::tokio_util::codec::LengthDelimitedCodec;

        let stream = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::UnixStream::connect(control_socket_path),
        )
        .await
        .map_err(|_| ControlClientError::Retryable {
            message: "connect control socket timed out".to_string(),
        })
        .and_then(|result| {
            result.map_err(|err| classify_io_error("connect control socket", err))
        })?;

        let framed = LengthDelimitedCodec::builder().new_framed(stream);
        let transport = serde_transport::new(framed, Bincode::default());
        let client = ControlPlaneClient::new(tarpc::client::Config::default(), transport).spawn();

        let request = OpenServiceRequest::new(service.to_string());
        let rpc_result = tokio::time::timeout(
            Duration::from_secs(5),
            client.open_service(context::current(), request),
        )
        .await
        .map_err(|_| ControlClientError::Retryable {
            message: "open_service request timed out".to_string(),
        })?;

        let service_result = rpc_result.map_err(|err| ControlClientError::Retryable {
            message: format!("open_service transport failed: {err}"),
        })?;

        service_result
            .map(|response| response.socket_path)
            .map_err(classify_control_error)
    })?;

    UnixStream::connect(&tunnel_socket_path)
        .map_err(|err| classify_io_error("connect service tunnel socket", err))
}

enum ControlClientError {
    Retryable { message: String },
    Fatal { message: String },
}

fn classify_control_error(err: bento_protocol::control::ControlError) -> ControlClientError {
    match err.code {
        ControlErrorCode::ServiceUnavailable | ControlErrorCode::InstanceNotRunning => {
            ControlClientError::Retryable {
                message: render_control_error(&err.code, &err.message),
            }
        }
        _ => ControlClientError::Fatal {
            message: render_control_error(&err.code, &err.message),
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

fn render_control_error(code: &ControlErrorCode, message: &str) -> String {
    match code {
        ControlErrorCode::ServiceUnavailable => {
            format!("service_unavailable: {message}. ensure guest service is running")
        }
        ControlErrorCode::UnknownService => {
            format!("unknown_service: {message}. try a supported service like 'ssh'")
        }
        ControlErrorCode::UnsupportedVersion => {
            format!(
                "unsupported_version: {message}. update bentoctl/instanced to matching versions"
            )
        }
        ControlErrorCode::UnsupportedRequest => {
            format!("unsupported_request: {message}")
        }
        ControlErrorCode::InstanceNotRunning => {
            format!("instance_not_running: {message}")
        }
        ControlErrorCode::PermissionDenied => {
            format!("permission_denied: {message}")
        }
        ControlErrorCode::Internal => {
            format!("internal_error: {message}")
        }
    }
}

fn proxy_stdio(mut stream: UnixStream) -> eyre::Result<()> {
    let mut stream_write = stream.try_clone().context("clone relay stream")?;
    let copy_in = std::thread::spawn(move || -> std::io::Result<()> {
        let stdin_fd = std::io::stdin().as_fd().try_clone_to_owned()?;
        let mut stdin_file = std::fs::File::from(stdin_fd);
        std::io::copy(&mut stdin_file, &mut stream_write)?;
        let _ = stream_write.shutdown(std::net::Shutdown::Write);
        Ok(())
    });

    let stdout_fd = std::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .context("dup stdout fd")?;

    let mut stdout_file = std::fs::File::from(stdout_fd);
    let _ = std::io::copy(&mut stream, &mut stdout_file).context("relay shell output")?;
    stdout_file.flush().context("flush shell output")?;

    match copy_in.join() {
        Ok(Ok(_in_bytes)) => Ok(()),
        Ok(Err(err)) => Err(err).context("relay shell input"),
        Err(_) => bail!("shell relay thread panicked"),
    }
}
