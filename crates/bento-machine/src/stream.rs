use std::fmt;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(target_os = "linux")]
use bento_fc::{SerialConnection as FcSerialConnection, VsockConnection as FcVsockConnection};
#[cfg(target_os = "macos")]
use bento_vz::device::{
    SerialPortStream as VzSerialStream, VirtioSocketConnection as VzVsockConnection,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct VsockStream {
    inner: VsockStreamInner,
}

pub struct SerialStream {
    inner: SerialStreamInner,
}

enum VsockStreamInner {
    #[cfg(target_os = "linux")]
    Firecracker(FcVsockConnection),
    #[cfg(target_os = "macos")]
    Vz(VzVsockConnection),
}

enum SerialStreamInner {
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

impl fmt::Debug for SerialStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerialStream").finish_non_exhaustive()
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

    pub fn source_port(&self) -> Option<u32> {
        match &self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => stream.source_port(),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Some(stream.source_port()),
        }
    }

    pub fn destination_port(&self) -> u32 {
        match &self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => stream.destination_port(),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => stream.destination_port(),
        }
    }
}

impl SerialStream {
    #[cfg(target_os = "linux")]
    pub(crate) fn from_firecracker(stream: FcSerialConnection) -> Self {
        Self {
            inner: SerialStreamInner::Firecracker(stream),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn from_vz(stream: VzSerialStream) -> Self {
        Self {
            inner: SerialStreamInner::Vz(stream),
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
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

impl AsyncRead for SerialStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            SerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(target_os = "macos")]
            SerialStreamInner::Vz(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for SerialStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            SerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(target_os = "macos")]
            SerialStreamInner::Vz(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            SerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(target_os = "macos")]
            SerialStreamInner::Vz(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(target_os = "linux")]
            SerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(target_os = "macos")]
            SerialStreamInner::Vz(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}
