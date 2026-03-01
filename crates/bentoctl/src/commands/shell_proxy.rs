use std::fmt::{Display, Formatter};
use std::io::{self, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use bento_runtime::instance::{InstanceFile, InstanceStatus};
use bento_runtime::instance_control::{
    ControlErrorCode, ControlRequest, ControlResponse, ControlResponseBody,
    CONTROL_PROTOCOL_VERSION, SERVICE_SSH,
};
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
        let socket_path = inst.file(InstanceFile::InstancedSocket);

        let should_wait_for_guest_readiness =
            inst.status() == InstanceStatus::Running && inst.expects_guest_agent();
        let deadline = Instant::now() + service_readiness::DEFAULT_SERVICE_READINESS_TIMEOUT;

        loop {
            match try_open_service_once(&socket_path, &self.service) {
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
                        &socket_path,
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

fn try_open_service_once(
    socket_path: &std::path::Path,
    service: &str,
) -> Result<UnixStream, ControlClientError> {
    let client = ControlClient::connect(socket_path)?;
    client.open_service(service)
}

struct ControlClient {
    stream: UnixStream,
}

impl ControlClient {
    fn connect(path: &std::path::Path) -> Result<Self, ControlClientError> {
        let stream = match UnixStream::connect(path) {
            Ok(stream) => stream,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(ControlClientError::Retryable {
                    message: format!(
                        "instanced_unreachable: control socket {} is missing, make sure the VM is running",
                        path.display()
                    ),
                });
            }
            Err(err) => {
                return Err(classify_io_error(
                    "connect control socket",
                    io::Error::new(err.kind(), format!("{} ({})", err, path.display())),
                ))
            }
        };

        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|err| classify_io_error("set control socket read timeout", err))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|err| classify_io_error("set control socket write timeout", err))?;

        Ok(Self { stream })
    }

    fn open_service(mut self, service: &str) -> Result<UnixStream, ControlClientError> {
        let request = ControlRequest::v1_open_service("shell-proxy", service);
        request
            .write_to(&mut self.stream)
            .map_err(|err| classify_io_error("write shell request", err))?;

        loop {
            let response = ControlResponse::read_from(&mut self.stream)
                .map_err(|err| classify_io_error("read shell response", err))?;

            if response.version != CONTROL_PROTOCOL_VERSION {
                return Err(ControlClientError::Fatal {
                    message: format!(
                        "unsupported_version: daemon returned protocol version {}, expected {}",
                        response.version, CONTROL_PROTOCOL_VERSION
                    ),
                });
            }

            match response.body {
                ControlResponseBody::Opened => {
                    self.stream.set_read_timeout(None).map_err(|err| {
                        classify_io_error("clear control socket read timeout", err)
                    })?;
                    self.stream.set_write_timeout(None).map_err(|err| {
                        classify_io_error("clear control socket write timeout", err)
                    })?;
                    return Ok(self.stream);
                }
                ControlResponseBody::Starting { .. } => {
                    continue;
                }
                ControlResponseBody::Error { code, message } => {
                    return Err(classify_control_error(&code, &message));
                }
                ControlResponseBody::Services { .. } => {
                    return Err(ControlClientError::Fatal {
                        message: "invalid_response: expected opened response for service request"
                            .to_string(),
                    });
                }
            }
        }
    }
}

enum ControlClientError {
    Retryable { message: String },
    Fatal { message: String },
}

fn classify_control_error(code: &ControlErrorCode, message: &str) -> ControlClientError {
    match code {
        ControlErrorCode::ServiceUnavailable | ControlErrorCode::InstanceNotRunning => {
            ControlClientError::Retryable {
                message: render_control_error(code, message),
            }
        }
        _ => ControlClientError::Fatal {
            message: render_control_error(code, message),
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
