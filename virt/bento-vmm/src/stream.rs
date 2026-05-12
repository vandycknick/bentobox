use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::pin::Pin;
use std::task::{ready, Context, Poll};

#[cfg(target_os = "linux")]
use bento_fc::{SerialConnection as FcSerialConnection, VsockConnection as FcVsockConnection};
#[cfg(target_os = "macos")]
use bento_vz::device::{
    SerialPortStream as VzSerialStream, VirtioSocketConnection as VzVsockConnection,
    VirtioSocketListener as VzVsockListener,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[cfg(unix)]
use tokio::net::{UnixListener as TokioUnixListener, UnixStream as TokioUnixStream};

pub struct VsockStream {
    inner: VsockStreamInner,
}

pub struct VsockListener {
    inner: VsockListenerInner,
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
    #[allow(dead_code)]
    #[cfg(unix)]
    SplitFile(SplitFileStream),
    #[allow(dead_code)]
    #[cfg(unix)]
    Unix(TokioUnixStream),
    #[cfg(target_os = "linux")]
    Firecracker(FcSerialConnection),
    #[cfg(target_os = "macos")]
    Vz(VzSerialStream),
}

enum VsockListenerInner {
    #[allow(dead_code)]
    #[cfg(unix)]
    Unix(TokioUnixListener),
    #[cfg(target_os = "macos")]
    Vz(VzVsockListener),
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

impl fmt::Debug for VsockListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VsockListener").finish_non_exhaustive()
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
    #[allow(dead_code)]
    pub(crate) fn from_unix_stream(stream: TokioUnixStream) -> Self {
        Self {
            inner: VsockStreamInner::Unix(stream),
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

    #[cfg(unix)]
    pub fn dup_fd(&self) -> io::Result<OwnedFd> {
        let fd = match &self.inner {
            #[cfg(target_os = "linux")]
            VsockStreamInner::Firecracker(stream) => duplicate_nonblocking_fd(stream)?,
            #[cfg(unix)]
            VsockStreamInner::Unix(stream) => duplicate_nonblocking_fd(stream)?,
            #[cfg(target_os = "macos")]
            VsockStreamInner::Vz(stream) => duplicate_nonblocking_fd(stream)?,
        };

        Ok(fd)
    }
}

impl VsockListener {
    #[cfg(unix)]
    #[allow(dead_code)]
    pub(crate) fn from_unix_listener(listener: TokioUnixListener) -> Self {
        Self {
            inner: VsockListenerInner::Unix(listener),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn from_vz(listener: VzVsockListener) -> Self {
        Self {
            inner: VsockListenerInner::Vz(listener),
        }
    }

    /// Wait for the next guest-initiated vsock connection.
    ///
    /// Returns the next available connection for this listener.
    pub async fn accept(&mut self) -> io::Result<VsockStream> {
        match &mut self.inner {
            #[cfg(unix)]
            VsockListenerInner::Unix(listener) => {
                listener.accept().await.map(|(stream, _)| VsockStream {
                    inner: VsockStreamInner::Unix(stream),
                })
            }
            #[cfg(target_os = "macos")]
            VsockListenerInner::Vz(listener) => listener
                .accept()
                .await
                .map(VsockStream::from_vz)
                .map_err(io::Error::other),
        }
    }

    /// Attempt to accept a queued connection without waiting.
    ///
    /// Returns `Ok(None)` if no connection is currently available.
    pub fn try_accept(&mut self) -> io::Result<Option<VsockStream>> {
        match &mut self.inner {
            #[cfg(unix)]
            VsockListenerInner::Unix(listener) => try_accept_unix(listener).map(|stream| {
                stream.map(|stream| VsockStream {
                    inner: VsockStreamInner::Unix(stream),
                })
            }),
            #[cfg(target_os = "macos")]
            VsockListenerInner::Vz(listener) => listener
                .try_accept()
                .map(|stream| stream.map(VsockStream::from_vz))
                .map_err(io::Error::other),
        }
    }
}

#[cfg(unix)]
fn try_accept_unix(listener: &TokioUnixListener) -> io::Result<Option<TokioUnixStream>> {
    use nix::errno::Errno;
    use nix::sys::socket::accept;

    match accept(listener.as_raw_fd()) {
        Ok(fd) => {
            let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
            stream.set_nonblocking(true)?;
            TokioUnixStream::from_std(stream).map(Some)
        }
        Err(Errno::EAGAIN) => Ok(None),
        Err(err) => Err(io::Error::other(err)),
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct SplitFileStream {
    read: tokio::io::unix::AsyncFd<File>,
    write: tokio::io::unix::AsyncFd<File>,
}

#[cfg(unix)]
impl SplitFileStream {
    #[allow(dead_code)]
    fn new(read: File, write: File) -> io::Result<Self> {
        set_nonblocking(&read)?;
        set_nonblocking(&write)?;
        Ok(Self {
            read: tokio::io::unix::AsyncFd::new(read)?,
            write: tokio::io::unix::AsyncFd::new(write)?,
        })
    }
}

#[cfg(unix)]
impl AsyncRead for SplitFileStream {
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

#[cfg(unix)]
impl AsyncWrite for SplitFileStream {
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

#[cfg(unix)]
fn shutdown_write<F: AsRawFd>(file: &F) -> io::Result<()> {
    match nix::sys::socket::shutdown(file.as_raw_fd(), nix::sys::socket::Shutdown::Write) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ENOTSOCK | nix::errno::Errno::ENOTCONN) => Ok(()),
        Err(err) => Err(io::Error::other(format!("shutdown(SHUT_WR) failed: {err}"))),
    }
}

impl MachineSerialStream {
    #[allow(dead_code)]
    #[cfg(unix)]
    pub(crate) fn from_files(read: File, write: File) -> io::Result<Self> {
        Ok(Self {
            inner: MachineSerialStreamInner::SplitFile(SplitFileStream::new(read, write)?),
        })
    }

    #[allow(dead_code)]
    #[cfg(unix)]
    pub(crate) fn from_unix_stream(stream: TokioUnixStream) -> Self {
        Self {
            inner: MachineSerialStreamInner::Unix(stream),
        }
    }

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
            #[cfg(unix)]
            MachineSerialStreamInner::SplitFile(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(unix)]
            MachineSerialStreamInner::Unix(stream) => Pin::new(stream).poll_read(cx, buf),
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
            #[cfg(unix)]
            MachineSerialStreamInner::SplitFile(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(unix)]
            MachineSerialStreamInner::Unix(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(target_os = "linux")]
            MachineSerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(target_os = "macos")]
            MachineSerialStreamInner::Vz(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(unix)]
            MachineSerialStreamInner::SplitFile(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(unix)]
            MachineSerialStreamInner::Unix(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(target_os = "linux")]
            MachineSerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(target_os = "macos")]
            MachineSerialStreamInner::Vz(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            #[cfg(unix)]
            MachineSerialStreamInner::SplitFile(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(unix)]
            MachineSerialStreamInner::Unix(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(target_os = "linux")]
            MachineSerialStreamInner::Firecracker(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(target_os = "macos")]
            MachineSerialStreamInner::Vz(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

#[cfg(unix)]
fn duplicate_nonblocking_fd<F>(fd_owner: &F) -> io::Result<OwnedFd>
where
    F: AsRawFd,
{
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd_owner.as_raw_fd()) };
    let duplicated = nix::unistd::dup(borrowed).map_err(io::Error::other)?;
    let file = std::fs::File::from(duplicated);
    set_nonblocking(&file)?;
    Ok(file.into())
}

#[cfg(unix)]
fn set_nonblocking(file: &std::fs::File) -> io::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};

    let flags =
        OFlag::from_bits_truncate(fcntl(file, FcntlArg::F_GETFL).map_err(io::Error::other)?);
    let new_flags = flags | OFlag::O_NONBLOCK;
    let _ = fcntl(file, FcntlArg::F_SETFL(new_flags)).map_err(io::Error::other)?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::io;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use nix::libc;
    use tokio::net::{UnixListener, UnixStream};

    use crate::stream::{VsockListener, VsockStream};

    fn temp_socket_path(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        PathBuf::from("/tmp").join(format!("bv-{name}-{}-{now}.sock", std::process::id()))
    }

    #[tokio::test]
    async fn dup_fd_returns_valid_nonblocking_descriptor() {
        let (mut left, right) = StdUnixStream::pair().expect("unix stream pair should be created");
        right
            .set_nonblocking(true)
            .expect("right stream should be nonblocking");

        let file = unsafe { std::fs::File::from_raw_fd(right.into_raw_fd()) };
        let stream = VsockStream::from_file(file).expect("vsock stream should wrap unix stream");
        let duplicated = stream.dup_fd().expect("dup fd should succeed");

        let raw_flags = unsafe { libc::fcntl(duplicated.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(raw_flags, -1, "fcntl should succeed");
        assert_ne!(raw_flags & libc::O_NONBLOCK, 0, "fd should be nonblocking");

        let mut duplicated_stream = StdUnixStream::from(duplicated);
        left.write_all(b"ping").expect("write should succeed");

        let mut buf = [0u8; 4];
        loop {
            match duplicated_stream.read(&mut buf) {
                Ok(4) => break,
                Ok(_) => panic!("unexpected short read"),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
                Err(err) => panic!("read should succeed: {err}"),
            }
        }

        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn unix_listener_accepts_vsock_streams() {
        let path = temp_socket_path("accept");
        let listener = UnixListener::bind(&path).expect("listener should bind");
        let mut listener = VsockListener::from_unix_listener(listener);

        let client = tokio::spawn(UnixStream::connect(path.clone()));
        let accepted = listener.accept().await.expect("accept should succeed");
        let _client = client
            .await
            .expect("client task should complete")
            .expect("client should connect");

        assert_eq!(accepted.destination_port(), 0);
        let _ = std::fs::remove_file(path);
    }
}
