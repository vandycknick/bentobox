use bento_virt::VsockStream;
use std::io;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

pub fn spawn_tunnel(stream: UnixStream, vsock_stream: VsockStream) {
    tokio::spawn(async move {
        if let Err(err) = proxy_streams(stream, vsock_stream).await {
            if is_expected_disconnect(&err) {
                tracing::debug!(error = %err, "vsock relay closed");
            } else {
                tracing::error!(error = %err, "vsock relay failed");
            }
        }
    });
}

async fn proxy_streams(
    mut client_stream: UnixStream,
    mut vsock_stream: VsockStream,
) -> std::io::Result<()> {
    match tokio::io::copy_bidirectional(&mut client_stream, &mut vsock_stream).await {
        Ok(_) => {}
        Err(err) if is_expected_disconnect(&err) => return Ok(()),
        Err(err) => return Err(err),
    }

    match client_stream.shutdown().await {
        Ok(()) => Ok(()),
        Err(err) if is_expected_disconnect(&err) => Ok(()),
        Err(err) => Err(err),
    }
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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::time::Duration;

    use bento_virt::VsockStream;
    use tokio::net::UnixStream;

    use crate::net::tunnel::proxy_streams;

    #[tokio::test]
    async fn proxy_streams_exits_after_client_disconnect() {
        let (client_stream, peer_stream) =
            UnixStream::pair().expect("unix stream pair should be created");
        let (vsock_stream, guest_stream) =
            StdUnixStream::pair().expect("guest stream pair should be created");
        vsock_stream
            .set_nonblocking(true)
            .expect("vsock stream should be nonblocking");
        guest_stream
            .set_nonblocking(true)
            .expect("guest stream should be nonblocking");

        let vsock_stream =
            UnixStream::from_std(vsock_stream).expect("tokio stream should wrap std unix stream");
        let vsock_stream = VsockStream::from_unix_stream(vsock_stream);
        let tunnel = tokio::spawn(async move { proxy_streams(client_stream, vsock_stream).await });

        drop(peer_stream);
        drop(guest_stream);

        let result = tokio::time::timeout(Duration::from_secs(1), tunnel)
            .await
            .expect("proxy task should exit promptly")
            .expect("proxy task should join successfully");

        result.expect("proxy should treat disconnect as clean shutdown");
    }
}
