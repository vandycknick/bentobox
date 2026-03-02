use std::os::fd::{AsFd, AsRawFd};
use std::time::Duration;

use bento_protocol::control::{
    ControlErrorCode, ControlPlaneClient, OpenServiceRequest, SERVICE_SERIAL,
};
use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
use eyre::{bail, Context};

use tarpc::context;
use tarpc::serde_transport;
use tarpc::tokio_serde::formats::Bincode;
use tarpc::tokio_util::codec::LengthDelimitedCodec;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn attach_serial(name: &str) -> eyre::Result<()> {
    let manager = InstanceManager::new(NixDaemon::new("123"));
    let inst = manager.inspect(name)?;
    let socket_path = inst.file(InstanceFile::InstancedSocket);

    let control_stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::UnixStream::connect(&socket_path),
    )
    .await
    .map_err(|_| eyre::eyre!("connect control socket timed out"))?
    .map_err(|err| eyre::eyre!("connect {}: {err}", socket_path.display()))?;

    let framed = LengthDelimitedCodec::builder().new_framed(control_stream);
    let transport = serde_transport::new(framed, Bincode::default());
    let client = ControlPlaneClient::new(tarpc::client::Config::default(), transport).spawn();

    let request = OpenServiceRequest::new(SERVICE_SERIAL);
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client.open_service(context::current(), request),
    )
    .await
    .map_err(|_| eyre::eyre!("open serial service request timed out"))?
    .map_err(|err| eyre::eyre!("open serial service transport failed: {err}"))?
    .map_err(|err| eyre::eyre!("{}", render_control_error(&err.code, &err.message)))?;

    let tunnel_socket = response.socket_path;

    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::UnixStream::connect(&tunnel_socket),
    )
    .await
    .map_err(|_| eyre::eyre!("connect serial tunnel socket timed out"))?
    .context(format!("connect serial tunnel socket {}", tunnel_socket))?;

    proxy_serial_stdio(stream).await
}

fn render_control_error(code: &ControlErrorCode, message: &str) -> String {
    match code {
        ControlErrorCode::ServiceUnavailable => {
            format!("service_unavailable: {message}. ensure guest service is running")
        }
        ControlErrorCode::UnknownService => {
            format!("unknown_service: {message}. try a supported service like 'serial'")
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

async fn proxy_serial_stdio(stream: tokio::net::UnixStream) -> eyre::Result<()> {
    let _raw_terminal = RawTerminalGuard::new()?;
    let (mut stream_read, mut stream_write) = stream.into_split();

    let output = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        let _ = tokio::io::copy(&mut stream_read, &mut stdout).await?;
        stdout.flush().await
    });

    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 1024];

    loop {
        let n = stdin.read(&mut buf).await.context("relay serial input")?;
        if n == 0 {
            break;
        }

        let chunk = &buf[..n];
        if chunk.contains(&0x1d) {
            let filtered: Vec<u8> = chunk.iter().copied().filter(|b| *b != 0x1d).collect();
            if !filtered.is_empty() {
                stream_write
                    .write_all(&filtered)
                    .await
                    .context("relay serial input")?;
            }
            stream_write
                .shutdown()
                .await
                .context("relay serial input")?;
            break;
        }

        stream_write
            .write_all(chunk)
            .await
            .context("relay serial input")?;
    }

    match output.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err).context("relay serial output"),
        Err(err) => bail!("serial output task failed: {err}"),
    }
}

struct RawTerminalGuard {
    fd: std::os::fd::OwnedFd,
    original: libc::termios,
    enabled: bool,
}

impl RawTerminalGuard {
    fn new() -> eyre::Result<Self> {
        let stdin = std::io::stdin();
        let fd = stdin.as_fd().try_clone_to_owned().context("dup stdin fd")?;

        if unsafe { libc::isatty(fd.as_raw_fd()) } == 0 {
            return Ok(Self {
                fd,
                original: unsafe { std::mem::zeroed() },
                enabled: false,
            });
        }

        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd.as_raw_fd(), &mut original) } != 0 {
            return Err(std::io::Error::last_os_error()).context("tcgetattr stdin");
        }

        let mut raw = original;
        raw.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd.as_raw_fd(), libc::TCSAFLUSH, &raw) } != 0 {
            return Err(std::io::Error::last_os_error()).context("tcsetattr stdin raw");
        }

        Ok(Self {
            fd,
            original,
            enabled: true,
        })
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ =
                unsafe { libc::tcsetattr(self.fd.as_raw_fd(), libc::TCSAFLUSH, &self.original) };
        }
    }
}
