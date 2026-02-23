use std::fmt::{Display, Formatter};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_control::{
    ControlRequest, ControlResponse, CONTROL_OP_OPEN_VSOCK, CONTROL_PROTOCOL_VERSION,
};
use bento_runtime::instance_manager::{InstanceManager, NixDaemon};
use clap::Args;
use eyre::{bail, Context};

#[derive(Args, Debug)]
#[command(hide = true)]
pub struct Cmd {
    #[arg(long)]
    pub name: String,

    #[arg(long, default_value_t = 2222)]
    pub port: u32,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "--name {} --port {}", self.name, self.port)
    }
}

impl Cmd {
    pub fn run(&self) -> eyre::Result<()> {
        let manager = InstanceManager::new(NixDaemon::new("123"));
        let inst = manager.inspect(&self.name)?;
        let socket_path = inst.file(InstanceFile::InstancedSocket);

        let mut stream = match UnixStream::connect(&socket_path) {
            Ok(stream) => stream,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                bail!(
                    "instanced_unreachable: control socket {} is missing, make sure the VM is running",
                    socket_path.display()
                )
            }
            Err(err) => {
                return Err(err).context(format!("connect {}", socket_path.display()));
            }
        };
        stream
            .set_nonblocking(false)
            .context("set control socket blocking")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .context("set control socket read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .context("set control socket write timeout")?;

        let request = ControlRequest {
            version: CONTROL_PROTOCOL_VERSION,
            id: "shell-proxy".to_string(),
            op: CONTROL_OP_OPEN_VSOCK.to_string(),
            port: Some(self.port),
        };
        let mut payload = serde_json::to_vec(&request).context("serialize shell request")?;
        payload.push(b'\n');
        stream.write_all(&payload).context("write shell request")?;
        stream.flush().context("flush shell request")?;

        let line = read_json_line(&mut stream).context("read shell response")?;
        if line.is_empty() {
            bail!("instanced closed control socket before sending response");
        }

        let response =
            serde_json::from_str::<ControlResponse>(&line).context("parse shell response")?;
        if !response.ok {
            let code = response.code.unwrap_or_else(|| "unknown_error".to_string());
            let msg = response
                .message
                .unwrap_or_else(|| "shell request rejected".to_string());
            bail!("{}", render_control_error(&code, &msg));
        }

        stream
            .set_read_timeout(None)
            .context("clear control socket read timeout")?;
        stream
            .set_write_timeout(None)
            .context("clear control socket write timeout")?;

        proxy_stdio(stream)
    }
}

fn render_control_error(code: &str, message: &str) -> String {
    match code {
        "guest_port_unreachable" => {
            format!(
                "guest_port_unreachable: {message}. ensure the guest VSOCK bridge service is running"
            )
        }
        "unsupported_version" => {
            format!(
                "unsupported_version: {message}. update bentoctl/instanced to matching versions"
            )
        }
        "missing_port" | "unsupported_op" => format!("{code}: {message}"),
        _ => format!("{code}: {message}"),
    }
}

fn read_json_line(stream: &mut UnixStream) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let n = stream.read(&mut byte)?;
        if n == 0 {
            break;
        }

        if byte[0] == b'\n' {
            break;
        }

        buf.push(byte[0]);
        if buf.len() > 16 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "control line exceeded 16KiB",
            ));
        }
    }

    String::from_utf8(buf).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "control line was not utf-8",
        )
    })
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
