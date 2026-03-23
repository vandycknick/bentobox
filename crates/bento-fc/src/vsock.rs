use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::FirecrackerError;

const CONNECT_ACK_PREFIX: &str = "OK ";

#[derive(Clone, Debug)]
pub struct VsockDevice {
    guest_cid: u32,
    uds_path: PathBuf,
}

impl VsockDevice {
    pub(crate) fn new(guest_cid: u32, uds_path: PathBuf) -> Self {
        Self {
            guest_cid,
            uds_path,
        }
    }

    pub fn guest_cid(&self) -> u32 {
        self.guest_cid
    }

    pub fn uds_path(&self) -> &Path {
        &self.uds_path
    }

    pub async fn connect(&self, port: u32) -> Result<VsockConnection, FirecrackerError> {
        let mut stream = tokio::net::UnixStream::connect(&self.uds_path).await?;
        let request = format!("CONNECT {port}\n");
        stream.write_all(request.as_bytes()).await?;

        let source_port = read_connect_ack(&mut stream).await?;
        let stream = stream.into_std()?;
        stream.set_nonblocking(true)?;

        Ok(VsockConnection {
            stream,
            source_port: Some(source_port),
            destination_port: port,
        })
    }

    pub fn bind(&self, port: u32) -> Result<VsockListener, FirecrackerError> {
        let socket_path = listener_path(&self.uds_path, port);
        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        Ok(VsockListener {
            listener,
            destination_port: port,
            socket_path,
        })
    }
}

#[derive(Debug)]
pub struct VsockConnection {
    stream: UnixStream,
    source_port: Option<u32>,
    destination_port: u32,
}

impl VsockConnection {
    pub fn source_port(&self) -> Option<u32> {
        self.source_port
    }

    pub fn destination_port(&self) -> u32 {
        self.destination_port
    }
}

impl AsRawFd for VsockConnection {
    fn as_raw_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }
}

impl Read for VsockConnection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(buf)
    }
}

impl Read for &VsockConnection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&self.stream).read(buf)
    }
}

impl Write for VsockConnection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

impl Write for &VsockConnection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        (&self.stream).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (&self.stream).flush()
    }
}

#[derive(Debug)]
pub struct VsockListener {
    listener: UnixListener,
    destination_port: u32,
    socket_path: PathBuf,
}

impl VsockListener {
    pub fn accept(&self) -> Result<VsockConnection, FirecrackerError> {
        let (stream, _) = self.listener.accept()?;
        stream.set_nonblocking(true)?;
        Ok(VsockConnection {
            stream,
            source_port: None,
            destination_port: self.destination_port,
        })
    }

    pub fn destination_port(&self) -> u32 {
        self.destination_port
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl AsRawFd for VsockListener {
    fn as_raw_fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }
}

async fn read_connect_ack(stream: &mut tokio::net::UnixStream) -> Result<u32, FirecrackerError> {
    let mut response = Vec::new();
    loop {
        let mut byte = [0; 1];
        let read = stream.read(&mut byte).await?;
        if read == 0 {
            return Err(FirecrackerError::InvalidVsockHandshake(
                "connection closed before Firecracker sent an acknowledgement".to_string(),
            ));
        }

        response.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if response.len() > 64 {
            return Err(FirecrackerError::InvalidVsockHandshake(
                "acknowledgement line exceeded 64 bytes".to_string(),
            ));
        }
    }

    parse_connect_ack(&response)
}

fn parse_connect_ack(response: &[u8]) -> Result<u32, FirecrackerError> {
    let response = std::str::from_utf8(response).map_err(|_| {
        FirecrackerError::InvalidVsockHandshake("acknowledgement was not valid UTF-8".to_string())
    })?;
    let response = response.trim_end_matches('\n');
    let Some(source_port) = response.strip_prefix(CONNECT_ACK_PREFIX) else {
        return Err(FirecrackerError::InvalidVsockHandshake(format!(
            "expected acknowledgement starting with {CONNECT_ACK_PREFIX:?}, got {response:?}",
        )));
    };

    source_port.parse().map_err(|err| {
        FirecrackerError::InvalidVsockHandshake(format!(
            "failed to parse host-side source port {source_port:?}: {err}",
        ))
    })
}

fn listener_path(base_path: &Path, port: u32) -> PathBuf {
    let mut socket_path = base_path.as_os_str().to_os_string();
    socket_path.push(format!("_{port}"));
    PathBuf::from(socket_path)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{listener_path, parse_connect_ack};

    #[test]
    fn parse_connect_ack_returns_source_port() {
        let source_port = parse_connect_ack(b"OK 1073741824\n").expect("valid ack");
        assert_eq!(source_port, 1_073_741_824);
    }

    #[test]
    fn parse_connect_ack_rejects_invalid_prefix() {
        let error = parse_connect_ack(b"NOPE\n").expect_err("invalid ack");
        assert!(error.to_string().contains("expected acknowledgement"));
    }

    #[test]
    fn listener_path_appends_destination_port() {
        let path = listener_path(Path::new("/tmp/firecracker.vsock"), 52);
        assert_eq!(path, PathBuf::from("/tmp/firecracker.vsock_52"));
    }
}
