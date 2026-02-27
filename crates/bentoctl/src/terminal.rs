use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_control::{
    ControlErrorCode, ControlRequest, ControlResponse, ControlResponseBody,
    CONTROL_PROTOCOL_VERSION, SERVICE_SERIAL,
};
use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
use eyre::{bail, Context};
use serde_json::Map;

pub(crate) fn attach_serial(name: &str) -> eyre::Result<()> {
    let manager = InstanceManager::new(NixDaemon::new("123"));
    let inst = manager.inspect(name)?;
    let socket_path = inst.file(InstanceFile::InstancedSocket);

    let mut stream = match UnixStream::connect(&socket_path) {
        Ok(stream) => stream,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "instanced_unreachable: control socket {} is missing, make sure the VM is running",
                socket_path.display()
            )
        }
        Err(err) => return Err(err).context(format!("connect {}", socket_path.display())),
    };

    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("set control socket read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .context("set control socket write timeout")?;

    let mut options = Map::new();
    options.insert(
        "access".to_string(),
        serde_json::Value::String("interactive".to_string()),
    );

    ControlRequest::v1_open_service_with_options("serial-attach", SERVICE_SERIAL, options)
        .write_to(&mut stream)
        .context("write serial request")?;

    loop {
        let response = ControlResponse::read_from(&mut stream).context("read serial response")?;

        if response.version != CONTROL_PROTOCOL_VERSION {
            bail!(
                "unsupported_version: daemon returned protocol version {}, expected {}",
                response.version,
                CONTROL_PROTOCOL_VERSION
            );
        }

        match response.body {
            ControlResponseBody::Opened => {
                stream
                    .set_read_timeout(None)
                    .context("clear control socket read timeout")?;
                stream
                    .set_write_timeout(None)
                    .context("clear control socket write timeout")?;
                return proxy_serial_stdio(stream);
            }
            ControlResponseBody::Starting { .. } => continue,
            ControlResponseBody::Error { code, message } => {
                bail!("{}", render_control_error(&code, &message))
            }
            ControlResponseBody::Services { .. } => {
                bail!("invalid_response: expected opened response for service request")
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

fn proxy_serial_stdio(mut stream: UnixStream) -> eyre::Result<()> {
    let _raw_terminal = RawTerminalGuard::new()?;

    let mut stream_write = stream.try_clone().context("clone serial relay stream")?;
    let input = std::thread::spawn(move || -> std::io::Result<()> {
        let stdin_fd = std::io::stdin().as_fd().try_clone_to_owned()?;
        let mut stdin_file = std::fs::File::from(stdin_fd);
        let mut buf = [0u8; 1024];

        loop {
            let n = stdin_file.read(&mut buf)?;
            if n == 0 {
                break;
            }

            let chunk = &buf[..n];
            if chunk.contains(&0x1d) {
                let filtered: Vec<u8> = chunk.iter().copied().filter(|b| *b != 0x1d).collect();
                if !filtered.is_empty() {
                    stream_write.write_all(&filtered)?;
                }
                let _ = stream_write.shutdown(std::net::Shutdown::Write);
                break;
            }

            stream_write.write_all(chunk)?;
        }

        Ok(())
    });

    let stdout_fd = std::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .context("dup stdout fd")?;
    let mut stdout_file = std::fs::File::from(stdout_fd);
    let _ = std::io::copy(&mut stream, &mut stdout_file).context("relay serial output")?;
    stdout_file.flush().context("flush serial output")?;

    match input.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err).context("relay serial input"),
        Err(_) => bail!("serial relay thread panicked"),
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
