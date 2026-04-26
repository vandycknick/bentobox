use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{UnixListener, UnixStream};

use crate::error::CloudHypervisorError;

#[derive(Clone, Debug)]
pub struct VsockDevice {
    guest_cid: u32,
    socket_path: PathBuf,
}

impl VsockDevice {
    pub(crate) fn new(guest_cid: u32, socket_path: PathBuf) -> Self {
        Self {
            guest_cid,
            socket_path,
        }
    }

    pub fn guest_cid(&self) -> u32 {
        self.guest_cid
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn connect(&self, port: u32) -> Result<VsockConnection, CloudHypervisorError> {
        let mut stream = UnixStream::connect(&self.socket_path).await?;
        let request = format!("CONNECT {port}\n");
        stream.write_all(request.as_bytes()).await?;

        Ok(VsockConnection {
            stream,
            source_port: None,
            destination_port: port,
        })
    }

    pub fn bind(&self, port: u32) -> Result<VsockListener, CloudHypervisorError> {
        let socket_path = listener_path(&self.socket_path, port);
        let listener = UnixListener::bind(&socket_path)?;

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

impl AsyncRead for VsockConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for VsockConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[derive(Debug)]
pub struct VsockListener {
    listener: UnixListener,
    destination_port: u32,
    socket_path: PathBuf,
}

impl VsockListener {
    pub async fn accept(&self) -> Result<VsockConnection, CloudHypervisorError> {
        let (stream, _) = self.listener.accept().await?;
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

fn listener_path(base_path: &Path, port: u32) -> PathBuf {
    let mut socket_path = base_path.as_os_str().to_os_string();
    socket_path.push(format!("_{port}"));
    PathBuf::from(socket_path)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::listener_path;

    #[test]
    fn listener_path_appends_destination_port() {
        let path = listener_path(Path::new("/tmp/cloud-hypervisor.vsock"), 52);
        assert_eq!(path, PathBuf::from("/tmp/cloud-hypervisor.vsock_52"));
    }
}
