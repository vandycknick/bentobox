use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::sys::socket::{shutdown, Shutdown};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::FirecrackerError;

pub struct SerialConnection {
    inner: SplitFdStream,
}

impl fmt::Debug for SerialConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerialConnection").finish_non_exhaustive()
    }
}

impl SerialConnection {
    pub(crate) fn new(read: File, write: File) -> io::Result<Self> {
        Ok(Self {
            inner: SplitFdStream::new(read, write)?,
        })
    }

    pub(crate) fn from_cloned_files(read: &File, write: &File) -> Result<Self, FirecrackerError> {
        Ok(Self::new(read.try_clone()?, write.try_clone()?)?)
    }

    pub fn read_fd(&self) -> RawFd {
        self.inner.read.get_ref().as_raw_fd()
    }

    pub fn write_fd(&self) -> RawFd {
        self.inner.write.get_ref().as_raw_fd()
    }
}

impl AsyncRead for SerialConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for SerialConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[derive(Debug)]
struct SplitFdStream {
    read: AsyncFd<File>,
    write: AsyncFd<File>,
}

impl SplitFdStream {
    fn new(read: File, write: File) -> io::Result<Self> {
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

fn set_nonblocking(file: &File) -> io::Result<()> {
    let flags = fcntl(file, FcntlArg::F_GETFL)
        .map(OFlag::from_bits_truncate)
        .map_err(|err| io::Error::other(format!("fcntl(F_GETFL) failed: {err}")))?;

    fcntl(file, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
        .map_err(|err| io::Error::other(format!("fcntl(F_SETFL, O_NONBLOCK) failed: {err}")))?;

    Ok(())
}
