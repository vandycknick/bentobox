use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::sys::socket::{shutdown, Shutdown};
use std::io;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug)]
pub(crate) struct AsyncFdStream {
    inner: AsyncFd<std::fs::File>,
}

impl AsyncFdStream {
    pub(crate) fn new(file: std::fs::File) -> io::Result<Self> {
        set_nonblocking(&file)?;
        Ok(Self {
            inner: AsyncFd::new(file)?,
        })
    }

    fn poll_read_priv(
        &self,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let bytes =
            unsafe { &mut *(buf.unfilled_mut() as *mut [std::mem::MaybeUninit<u8>] as *mut [u8]) };

        loop {
            let mut guard = futures::ready!(self.inner.poll_read_ready(cx))?;
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

    fn poll_write_priv(&self, cx: &mut TaskContext<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = futures::ready!(self.inner.poll_write_ready(cx))?;
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
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_read_priv(cx, buf)
    }
}

impl AsyncWrite for AsyncFdStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_priv(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.inner.get_ref().flush()?;
        shutdown_write(self.inner.get_ref())?;
        Poll::Ready(Ok(()))
    }
}

fn shutdown_write(file: &std::fs::File) -> io::Result<()> {
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
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;

    use crate::async_fd::AsyncFdStream;

    #[tokio::test]
    async fn shutdown_propagates_eof_for_socket_fds() {
        let (stream, mut peer) = StdUnixStream::pair().expect("socket pair should be created");
        peer.set_read_timeout(Some(Duration::from_secs(1)))
            .expect("peer read timeout should be configured");

        let file = unsafe { std::fs::File::from_raw_fd(stream.into_raw_fd()) };
        let mut async_stream =
            AsyncFdStream::new(file).expect("async fd stream should wrap socket");

        async_stream
            .shutdown()
            .await
            .expect("socket shutdown should succeed");

        let mut buf = [0u8; 1];
        let read = peer.read(&mut buf).expect("peer read should complete");
        assert_eq!(read, 0, "peer should observe EOF after shutdown");
    }

    #[tokio::test]
    async fn shutdown_ignores_non_socket_fds() {
        let (read_fd, write_fd) = nix::unistd::pipe().expect("pipe should be created");
        drop(read_fd);

        let file = unsafe { std::fs::File::from_raw_fd(write_fd.into_raw_fd()) };
        let mut async_stream = AsyncFdStream::new(file).expect("async fd stream should wrap pipe");

        async_stream
            .shutdown()
            .await
            .expect("non-socket shutdown should fall back cleanly");
    }
}
