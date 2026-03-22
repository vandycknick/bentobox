use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::sys::socket::{shutdown, Shutdown};
use std::fmt;
use std::io;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::net::UnixStream as StdUnixStream;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[cfg(target_os = "linux")]
use tokio::net::UnixStream;

pub(crate) trait StreamIo: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

impl<T> StreamIo for T where T: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

pub(crate) struct RawVsockConnection {
    inner: Box<dyn StreamIo>,
}

pub(crate) struct RawSerialConnection {
    inner: Box<dyn StreamIo>,
}

impl RawVsockConnection {
    pub(crate) fn new<T>(stream: T) -> Self
    where
        T: StreamIo,
    {
        Self {
            inner: Box::new(stream),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn from_file(file: std::fs::File) -> io::Result<Self> {
        Ok(Self::new(AsyncFdStream::new(file)?))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn from_unix(stream: StdUnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self::new(UnixStream::from_std(stream)?))
    }
}

impl RawSerialConnection {
    pub(crate) fn new<T>(stream: T) -> Self
    where
        T: StreamIo,
    {
        Self {
            inner: Box::new(stream),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn from_files(read: std::fs::File, write: std::fs::File) -> io::Result<Self> {
        Ok(Self::new(SplitFdStream::new(read, write)?))
    }
}

pub struct VsockStream {
    inner: Box<dyn StreamIo>,
}

impl fmt::Debug for VsockStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VsockStream").finish_non_exhaustive()
    }
}

impl VsockStream {
    pub(crate) fn new<T>(stream: T) -> Self
    where
        T: StreamIo,
    {
        Self {
            inner: Box::new(stream),
        }
    }

    pub fn from_file(file: std::fs::File) -> io::Result<Self> {
        Ok(Self::new(AsyncFdStream::new(file)?))
    }

    pub(crate) fn from_raw(raw: RawVsockConnection) -> io::Result<Self> {
        Ok(Self { inner: raw.inner })
    }
}

impl AsyncRead for VsockStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for VsockStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

pub struct SerialStream {
    inner: Box<dyn StreamIo>,
}

impl fmt::Debug for SerialStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerialStream").finish_non_exhaustive()
    }
}

impl SerialStream {
    pub(crate) fn new<T>(stream: T) -> Self
    where
        T: StreamIo,
    {
        Self {
            inner: Box::new(stream),
        }
    }

    pub fn from_files(read: std::fs::File, write: std::fs::File) -> io::Result<Self> {
        Ok(Self::new(SplitFdStream::new(read, write)?))
    }

    pub(crate) fn from_raw(raw: RawSerialConnection) -> io::Result<Self> {
        Ok(Self { inner: raw.inner })
    }
}

impl AsyncRead for SerialStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for SerialStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

#[derive(Debug)]
struct AsyncFdStream {
    inner: AsyncFd<std::fs::File>,
}

impl AsyncFdStream {
    fn new(file: std::fs::File) -> io::Result<Self> {
        set_nonblocking(&file)?;
        Ok(Self {
            inner: AsyncFd::new(file)?,
        })
    }

    fn poll_read_priv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let bytes =
            unsafe { &mut *(buf.unfilled_mut() as *mut [std::mem::MaybeUninit<u8>] as *mut [u8]) };

        loop {
            let mut guard = ready!(self.inner.poll_read_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().read(bytes)) {
                Ok(Ok(n)) => {
                    unsafe {
                        buf.assume_init(n);
                    }
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }

    fn poll_write_priv(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.inner.poll_write_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(Ok(n)) => return Poll::Ready(Ok(n)),
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }
}

impl AsyncRead for AsyncFdStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_read_priv(cx, buf)
    }
}

impl AsyncWrite for AsyncFdStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_priv(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.get_ref().flush()?;
        shutdown_write(self.inner.get_ref())?;
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
struct SplitFdStream {
    read: AsyncFd<std::fs::File>,
    write: AsyncFd<std::fs::File>,
}

impl SplitFdStream {
    fn new(read: std::fs::File, write: std::fs::File) -> io::Result<Self> {
        set_nonblocking(&read)?;
        set_nonblocking(&write)?;
        Ok(Self {
            read: AsyncFd::new(read)?,
            write: AsyncFd::new(write)?,
        })
    }
}

impl AsyncRead for SplitFdStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let bytes =
            unsafe { &mut *(buf.unfilled_mut() as *mut [std::mem::MaybeUninit<u8>] as *mut [u8]) };

        loop {
            let mut guard = ready!(self.read.poll_read_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().read(bytes)) {
                Ok(Ok(n)) => {
                    unsafe {
                        buf.assume_init(n);
                    }
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }
}

impl AsyncWrite for SplitFdStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.write.poll_write_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(Ok(n)) => return Poll::Ready(Ok(n)),
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.write.get_ref().flush()?;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.write.get_ref().flush()?;
        shutdown_write(self.write.get_ref())?;
        Poll::Ready(Ok(()))
    }
}

fn shutdown_write<F: AsRawFd>(file: &F) -> io::Result<()> {
    match shutdown(file.as_raw_fd(), Shutdown::Write) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ENOTSOCK | nix::errno::Errno::ENOTCONN) => Ok(()),
        Err(err) => Err(io::Error::other(format!("shutdown(SHUT_WR) failed: {err}"))),
    }
}

fn set_nonblocking(file: &std::fs::File) -> io::Result<()> {
    let flags = fcntl(file, FcntlArg::F_GETFL)
        .map(OFlag::from_bits_truncate)
        .map_err(|err| io::Error::other(format!("fcntl(F_GETFL) failed: {err}")))?;

    fcntl(file, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
        .map_err(|err| io::Error::other(format!("fcntl(F_SETFL, O_NONBLOCK) failed: {err}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::io::Write as _;
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::stream::{SerialStream, VsockStream};

    #[tokio::test]
    async fn vsock_shutdown_propagates_eof_for_socket_fds() {
        let (stream, mut peer) = StdUnixStream::pair().expect("socket pair should be created");
        peer.set_read_timeout(Some(Duration::from_secs(1)))
            .expect("peer read timeout should be configured");

        let file = unsafe { std::fs::File::from_raw_fd(stream.into_raw_fd()) };
        let mut async_stream =
            VsockStream::from_file(file).expect("vsock stream should wrap socket");

        async_stream
            .shutdown()
            .await
            .expect("socket shutdown should succeed");

        let mut buf = [0u8; 1];
        let read = peer.read(&mut buf).expect("peer read should complete");
        assert_eq!(read, 0, "peer should observe EOF after shutdown");
    }

    #[tokio::test]
    async fn serial_stream_reads_and_writes_over_split_fds() {
        let (mut guest_output_writer, guest_output_reader) =
            StdUnixStream::pair().expect("guest output pair should be created");
        let (guest_input_reader, mut guest_input_writer) =
            StdUnixStream::pair().expect("guest input pair should be created");

        let read_file = unsafe { std::fs::File::from_raw_fd(guest_output_reader.into_raw_fd()) };
        let write_file = unsafe { std::fs::File::from_raw_fd(guest_input_reader.into_raw_fd()) };
        let mut stream =
            SerialStream::from_files(read_file, write_file).expect("serial stream should wrap fds");

        guest_output_writer
            .write_all(b"hello")
            .expect("guest output write should succeed");

        let mut buf = [0u8; 5];
        stream
            .read_exact(&mut buf)
            .await
            .expect("serial stream should read guest output");
        assert_eq!(&buf, b"hello");

        stream
            .write_all(b"world")
            .await
            .expect("serial stream should write guest input");
        stream.flush().await.expect("serial flush should succeed");

        let mut peer_buf = [0u8; 5];
        guest_input_writer
            .read_exact(&mut peer_buf)
            .expect("guest input peer should read bytes");
        assert_eq!(&peer_buf, b"world");
    }
}
