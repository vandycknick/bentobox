use std::fmt::{Display, Formatter};
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_control::{
    ControlErrorCode, ControlRequest, ControlResponse, ControlResponseBody,
    CONTROL_PROTOCOL_VERSION, SERVICE_SSH,
};
use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
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

        let client = ControlClient::connect(&socket_path)?;
        let stream = client.open_service(&self.service)?;

        proxy_stdio(stream)
    }
}

struct ControlClient {
    stream: UnixStream,
}

impl ControlClient {
    fn connect(path: &std::path::Path) -> eyre::Result<Self> {
        let stream = match UnixStream::connect(path) {
            Ok(stream) => stream,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                bail!(
                    "instanced_unreachable: control socket {} is missing, make sure the VM is running",
                    path.display()
                )
            }
            Err(err) => return Err(err).context(format!("connect {}", path.display())),
        };

        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .context("set control socket read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .context("set control socket write timeout")?;

        Ok(Self { stream })
    }

    fn open_service(mut self, service: &str) -> eyre::Result<UnixStream> {
        let request = ControlRequest::v1_open_service("shell-proxy", service);
        request
            .write_to(&mut self.stream)
            .context("write shell request")?;

        loop {
            let response =
                ControlResponse::read_from(&mut self.stream).context("read shell response")?;

            if response.version != CONTROL_PROTOCOL_VERSION {
                bail!(
                    "unsupported_version: daemon returned protocol version {}, expected {}",
                    response.version,
                    CONTROL_PROTOCOL_VERSION
                );
            }

            match response.body {
                ControlResponseBody::Opened => {
                    self.stream
                        .set_read_timeout(None)
                        .context("clear control socket read timeout")?;
                    self.stream
                        .set_write_timeout(None)
                        .context("clear control socket write timeout")?;
                    return Ok(self.stream);
                }
                ControlResponseBody::Starting { .. } => {
                    continue;
                }
                ControlResponseBody::Error { code, message } => {
                    bail!("{}", render_control_error(&code, &message))
                }
                ControlResponseBody::Services { .. } => {
                    bail!("invalid_response: expected opened response for service request")
                }
            }
        }
    }
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
    let _ = std::io::copy(&mut stream, &mut stdout_file).context(
        "relay
     shell output",
    )?;
    stdout_file.flush().context("flush shell output")?;

    match copy_in.join() {
        Ok(Ok(_in_bytes)) => Ok(()),
        Ok(Err(err)) => Err(err).context("relay shell input"),
        Err(_) => bail!("shell relay thread panicked"),
    }
}
