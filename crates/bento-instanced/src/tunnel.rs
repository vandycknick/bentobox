use std::io;
use std::os::fd::OwnedFd;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use crate::async_fd::AsyncFdStream;

pub fn spawn_tunnel(stream: UnixStream, vsock_fd: OwnedFd) {
    tokio::spawn(async move {
        if let Err(err) = proxy_streams(stream, vsock_fd).await {
            if is_expected_disconnect(&err) {
                tracing::debug!(error = %err, "vsock relay closed");
            } else {
                tracing::error!(error = %err, "vsock relay failed");
            }
        }
    });
}

async fn proxy_streams(mut client_stream: UnixStream, vsock_fd: OwnedFd) -> std::io::Result<()> {
    let mut vsock_stream = AsyncFdStream::new(std::fs::File::from(vsock_fd))?;
    let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut vsock_stream).await?;
    client_stream.shutdown().await
}

fn is_expected_disconnect(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::Interrupted
    )
}
