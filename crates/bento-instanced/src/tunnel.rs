use std::io;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::thread;

pub fn spawn_tunnel(stream: UnixStream, vsock_fd: OwnedFd) {
    thread::spawn(move || {
        if let Err(err) = proxy_streams(stream, vsock_fd) {
            tracing::error!(error = %err, "vsock relay failed");
        }
    });
}

fn proxy_streams(mut client_stream: UnixStream, vsock_fd: OwnedFd) -> io::Result<()> {
    client_stream.set_nonblocking(false)?;

    let mut client_read = client_stream.try_clone()?;
    let mut vsock_stream = std::fs::File::from(vsock_fd);
    let mut vsock_write = vsock_stream.try_clone()?;

    let forward = thread::spawn(move || {
        let stdin_done = io::copy(&mut client_read, &mut vsock_write);
        let _ = vsock_write.flush();
        stdin_done
    });

    let _ = io::copy(&mut vsock_stream, &mut client_stream)?;
    let _ = client_stream.shutdown(std::net::Shutdown::Write);

    match forward.join() {
        Ok(_) => Ok(()),
        Err(_) => Err(io::Error::other("relay thread panicked")),
    }
}
