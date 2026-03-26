use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::io;
#[cfg(unix)]
use std::os::fd::OwnedFd;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(target_os = "linux")]
use bento_fc::{SerialConnection as FcSerialConnection, VsockConnection as FcVsockConnection};
#[cfg(target_os = "macos")]
use bento_vz::device::{
    SerialPortStream as VzSerialStream, VirtioSocketConnection as VzVsockConnection,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[cfg(unix)]
use tokio::net::UnixStream as TokioUnixStream;

pub struct VsockStream {
    inner: VsockStreamInner,
}

pub(crate) struct MachineSerialStream {
    inner: MachineSerialStreamInner,
}

enum VsockStreamInner {
    #[cfg(target_os = "linux")]
    Firecracker(FcVsockConnection),
    #[cfg(unix)]
    Unix(TokioUnixStream),
    #[cfg(target_os = "macos")]
    Vz(VzVsockConnection),
}

enum MachineSerialStreamInner {
    #[cfg(target_os = "linux")]
    Firecracker(FcSerialConnection),
    #[cfg(target_os = "macos")]
    Vz(VzSerialStream),
}

impl fmt::Debug for VsockStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VsockStream").finish_non_exhaustive()
    }
}

impl fmt::Debug for MachineSerialStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineSerialStream")
            .finish_non_exhaustive()
    }
}

impl VsockStream {
    #[cfg(target_os = "linux")]
    pub(crate) fn from_firecracker(stream: FcVsockConnection) -> Self {
        Self {
            inner: VsockStreamInner::Firecracker(stream),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn from_vz(stream: VzVsockConnection) -> Self {
        Self {
            inner: VsockStreamInner::Vz(stream),
        }
    }

    #[cfg(unix)]
    pub fn from_file(file: File) -> io::Result<Self> {
        let fd: OwnedFd = file.into();
        let stream = std::os::unix::net::UnixStream::from(fd);
        stream.set_nonblocking(true)?;
        let stream = TokioUnixStream::from_std(stream)?;
        Ok(Self {
            inner: VsockStreamInner::Unix(stream),
        })
    }

    pub fn source_port(&self) -> Option<u32> {
        match &self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => stream.source_port(),
            #[cfg(unix)]
            VsockStreamInner::Unix(_) => None,
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Some(stream.source_port()),
        }
    }

    pub fn destination_port(&self) -> u32 {
        match &self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => stream.destination_port(),
            #[cfg(unix)]
            VsockStreamInner::Unix(_) => 0,
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => stream.destination_port(),
        }
    }
}

impl MachineSerialStream {
    #[cfg(target_os = "linux")]
    pub(crate) fn from_firecracker(stream: FcSerialConnection) -> Self {
        Self {
            inner: MachineSerialStreamInner::Firecracker(stream),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn from_vz(stream: VzSerialStream) -> Self {
        Self {
            inner: MachineSerialStreamInner::Vz(stream),
        }
    }
}

impl AsyncRead for VsockStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(unix)]
            VsockStreamInner::Unix(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for VsockStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(unix)]
            VsockStreamInner::Unix(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(unix)]
            VsockStreamInner::Unix(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(unix)]
            VsockStreamInner::Unix(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

impl AsyncRead for MachineSerialStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            MachineSerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(target_os = "macos")]
            MachineSerialStreamInner::Vz(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MachineSerialStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            MachineSerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(target_os = "macos")]
            MachineSerialStreamInner::Vz(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            MachineSerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(target_os = "macos")]
            MachineSerialStreamInner::Vz(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            MachineSerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(target_os = "macos")]
            MachineSerialStreamInner::Vz(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}
