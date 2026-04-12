use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::task::{Context, Poll};

use nix::cmsg_space;
use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
use serde::{Deserialize, Serialize};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const FD_PASS_MAGIC: u32 = 0x4245_4e54;

#[derive(Debug, Deserialize)]
pub struct StartupMessage {
    pub api_version: u32,
    pub endpoint: String,
    pub mode: String,
    pub port: u32,
    pub fd: i32,
}

impl StartupMessage {
    pub fn expect_listen(&self) -> io::Result<()> {
        if self.api_version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported api_version {}", self.api_version),
            ));
        }
        if self.mode != "listen" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("expected listen mode, got {}", self.mode),
            ));
        }
        if self.fd != 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("expected fd 3, got {}", self.fd),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PluginEvent<'a> {
    Ready,
    Failed {
        message: &'a str,
    },
    EndpointStatus {
        active: bool,
        summary: &'a str,
        problems: &'a [&'a str],
    },
}

pub fn read_startup_message() -> io::Result<StartupMessage> {
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    serde_json::from_str(&line).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

pub fn emit_event(event: PluginEvent<'_>) -> io::Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, &event).map_err(io::Error::other)?;
    lock.write_all(b"\n")?;
    lock.flush()
}

pub fn recv_conn_fd(control: RawFd) -> io::Result<(OwnedFd, u64)> {
    let mut payload = [0_u8; std::mem::size_of::<BentoFdPassV1>()];
    let mut iov = [std::io::IoSliceMut::new(&mut payload)];
    let mut cmsg_buf = cmsg_space!([RawFd; 1]);

    let msg = recvmsg::<()>(control, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())
        .map_err(io::Error::other)?;
    if msg.bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "control socket closed",
        ));
    }

    let mut received_fd = None;
    for cmsg in msg.cmsgs().map_err(io::Error::other)? {
        if let ControlMessageOwned::ScmRights(fds) = cmsg {
            received_fd = fds.into_iter().next();
        }
    }

    let conn_fd = received_fd.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing fd in SCM_RIGHTS message",
        )
    })?;

    let frame = BentoFdPassV1::from_bytes(payload)?;
    let fd = unsafe { OwnedFd::from_raw_fd(conn_fd) };
    Ok((fd, frame.conn_id))
}

pub fn into_async_stream(fd: OwnedFd) -> io::Result<AsyncStream> {
    Ok(AsyncStream {
        inner: AsyncFd::new(File::from(fd))?,
    })
}

pub struct AsyncStream {
    inner: AsyncFd<File>,
}

impl AsyncRead for AsyncStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        loop {
            let mut guard = match me.inner.poll_read_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            };

            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| inner.get_ref().read(unfilled)) {
                Ok(Ok(read)) => {
                    buf.advance(read);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for AsyncStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        loop {
            let mut guard = match me.inner.poll_write_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            };

            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[repr(C)]
struct BentoFdPassV1 {
    magic: u32,
    flags: u32,
    conn_id: u64,
}

impl BentoFdPassV1 {
    fn from_bytes(bytes: [u8; std::mem::size_of::<BentoFdPassV1>()]) -> io::Result<Self> {
        let magic = u32::from_ne_bytes(bytes[0..4].try_into().expect("slice is four bytes"));
        if magic != FD_PASS_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected fd frame magic {magic:#x}"),
            ));
        }

        Ok(Self {
            magic,
            flags: u32::from_ne_bytes(bytes[4..8].try_into().expect("slice is four bytes")),
            conn_id: u64::from_ne_bytes(bytes[8..16].try_into().expect("slice is eight bytes")),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    use std::os::unix::net::UnixStream as StdUnixStream;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::into_async_stream;

    #[tokio::test]
    async fn async_stream_reads_and_writes() {
        let (mut left, right) = StdUnixStream::pair().expect("unix stream pair should exist");
        right
            .set_nonblocking(true)
            .expect("right stream should be nonblocking");

        let fd = unsafe { OwnedFd::from_raw_fd(right.into_raw_fd()) };
        let mut stream = into_async_stream(fd).expect("wrap async stream");

        left.write_all(b"ping").expect("write should succeed");

        let mut buf = [0_u8; 4];
        stream
            .read_exact(&mut buf)
            .await
            .expect("read should succeed");
        assert_eq!(&buf, b"ping");

        stream
            .write_all(b"pong")
            .await
            .expect("write back should succeed");
        let mut peer_buf = [0_u8; 4];
        left.read_exact(&mut peer_buf)
            .expect("peer read should succeed");
        assert_eq!(&peer_buf, b"pong");
    }
}
