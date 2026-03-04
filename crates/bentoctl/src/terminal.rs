use std::os::fd::{AsFd, AsRawFd};

use bento_runtime::negotiate::{
    ClientUpgradeStreamError, Negotiate, ProxyMode, RejectCode, Upgrade,
};
use bento_runtime::services::SERVICE_SERIAL;
use eyre::{bail, Context};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub(crate) async fn attach_serial(socket_path: &str) -> eyre::Result<()> {
    let stream = match UnixStream::connect(socket_path).await {
        Ok(stream) => stream,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "instanced_unreachable: control socket {} is missing, make sure the VM is running",
                socket_path
            )
        }
        Err(err) => return Err(err).context(format!("connect {}", socket_path)),
    };

    let mode = ProxyMode::ReadWrite;
    let stream = Negotiate::client_upgrade_stream_v1(
        stream,
        Upgrade::Proxy {
            service: SERVICE_SERIAL.to_string(),
            mode,
        },
    )
    .await
    .map_err(|err| match err {
        ClientUpgradeStreamError::Io(io_err) => {
            eyre::eyre!(io_err).wrap_err("negotiate serial proxy stream")
        }
        ClientUpgradeStreamError::Reject(reject) => {
            eyre::eyre!(render_reject_error(reject.code, &reject.message))
        }
    })?;

    print_serial_exit_hint(mode);
    proxy_serial_stdio(stream).await
}

fn print_serial_exit_hint(mode: ProxyMode) {
    if mode != ProxyMode::ReadWrite {
        return;
    }

    if unsafe { libc::isatty(std::io::stderr().as_raw_fd()) } == 0 {
        return;
    }

    eprintln!("Connected to serial console. Exit with Ctrl+]");
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
            format!("unknown_service: {message}. try a supported service like 'serial'")
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

async fn proxy_serial_stdio(stream: UnixStream) -> eyre::Result<()> {
    let _raw_terminal = RawTerminalGuard::new()?;

    let (mut stream_read, mut stream_write) = stream.into_split();

    let input = async {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1024];

        loop {
            let n = stdin
                .read(&mut buf)
                .await
                .context("relay serial input read")?;
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
                        .context("relay serial input write")?;
                }
                stream_write
                    .shutdown()
                    .await
                    .context("relay serial input shutdown")?;
                break;
            }

            stream_write
                .write_all(chunk)
                .await
                .context("relay serial input write")?;
        }

        Ok::<(), eyre::Report>(())
    };

    let output = async {
        let mut stdout = tokio::io::stdout();
        tokio::io::copy(&mut stream_read, &mut stdout)
            .await
            .context("relay serial output")?;
        stdout.flush().await.context("flush serial output")?;
        Ok::<(), eyre::Report>(())
    };

    tokio::try_join!(output, input)?;
    Ok(())
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
