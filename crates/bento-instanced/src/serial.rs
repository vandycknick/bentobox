use bento_machine::{MachineHandle, OpenDeviceRequest, OpenDeviceResponse};
use bento_runtime::instance::InstanceFile;
use eyre::Context;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, Mutex};

use crate::async_fd::AsyncFdStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SerialAccess {
    Interactive,
    Watch,
}

#[derive(Debug)]
struct SerialHub {
    next_id: u64,
    interactive_owner: Option<u64>,
}

impl SerialHub {
    fn new() -> Self {
        Self {
            next_id: 1,
            interactive_owner: None,
        }
    }

    fn attach(&mut self, access: SerialAccess) -> eyre::Result<u64> {
        if access == SerialAccess::Interactive && self.interactive_owner.is_some() {
            eyre::bail!("interactive serial client is already attached");
        }

        let id = self.next_id;
        self.next_id += 1;

        if access == SerialAccess::Interactive {
            self.interactive_owner = Some(id);
        }

        Ok(id)
    }

    fn detach(&mut self, id: u64) {
        if self.interactive_owner == Some(id) {
            self.interactive_owner = None;
        }
    }

    fn can_write_input(&self, id: u64) -> bool {
        self.interactive_owner == Some(id)
    }
}

#[derive(Debug)]
pub(crate) struct SerialRuntime {
    hub: Arc<Mutex<SerialHub>>,
    guest_input: Arc<Mutex<AsyncFdStream>>,
    output_tx: broadcast::Sender<Vec<u8>>,
}

pub(crate) async fn create_serial_runtime(
    inst: &bento_runtime::instance::Instance,
    machine: &MachineHandle,
) -> eyre::Result<Arc<SerialRuntime>> {
    let device = machine
        .open_device(OpenDeviceRequest::Serial)
        .await
        .context("open serial device")?;

    let (guest_input, guest_output) = match device {
        OpenDeviceResponse::Serial {
            guest_input,
            guest_output,
        } => (guest_input, guest_output),
        OpenDeviceResponse::Vsock { .. } => {
            eyre::bail!("driver returned unexpected device type when opening serial")
        }
    };

    let serial_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(inst.file(InstanceFile::SerialLog))
        .context("open serial.log")?;

    let (output_tx, _) = broadcast::channel(256);
    let runtime = Arc::new(SerialRuntime {
        hub: Arc::new(Mutex::new(SerialHub::new())),
        guest_input: Arc::new(Mutex::new(AsyncFdStream::new(std::fs::File::from(
            guest_input,
        ))?)),
        output_tx,
    });

    spawn_serial_reader(
        AsyncFdStream::new(std::fs::File::from(guest_output)).context("wrap serial output fd")?,
        tokio::fs::File::from_std(serial_log),
        runtime.output_tx.clone(),
    );

    Ok(runtime)
}

fn spawn_serial_reader(
    mut guest_output: AsyncFdStream,
    mut serial_log: tokio::fs::File,
    output_tx: broadcast::Sender<Vec<u8>>,
) {
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];

        loop {
            let n = match guest_output.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    tracing::error!(error = %err, "serial read failed");
                    break;
                }
            };

            let chunk = &buf[..n];

            if let Err(err) = serial_log.write_all(chunk).await {
                tracing::error!(error = %err, "serial log write failed");
            }

            let _ = serial_log.flush().await;
            let _ = output_tx.send(chunk.to_vec());
        }
    });
}

pub(crate) fn spawn_serial_tunnel(
    stream: UnixStream,
    runtime: Arc<SerialRuntime>,
    access: SerialAccess,
) {
    tokio::spawn(async move {
        if let Err(err) = proxy_serial_stream(stream, runtime, access).await {
            if is_expected_disconnect(&err) {
                tracing::debug!(error = %err, "serial relay closed");
            } else {
                tracing::error!(error = %err, "serial relay failed");
            }
        }
    });
}

async fn proxy_serial_stream(
    client_stream: UnixStream,
    runtime: Arc<SerialRuntime>,
    access: SerialAccess,
) -> io::Result<()> {
    let client_id = {
        let mut hub = runtime.hub.lock().await;
        hub.attach(access)
            .map_err(|err| io::Error::other(format!("{err}")))?
    };

    let mut output_rx = runtime.output_tx.subscribe();
    let (mut client_read, mut client_write) = client_stream.into_split();

    let output_task: tokio::task::JoinHandle<io::Result<()>> = tokio::spawn(async move {
        loop {
            match output_rx.recv().await {
                Ok(chunk) => {
                    client_write.write_all(&chunk).await?;
                    client_write.flush().await?;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    });

    let relay_result = match access {
        SerialAccess::Interactive => {
            relay_client_input(client_id, runtime.clone(), &mut client_read).await
        }
        SerialAccess::Watch => wait_for_client_disconnect(&mut client_read).await,
    };

    {
        let mut hub = runtime.hub.lock().await;
        hub.detach(client_id);
    }

    output_task.abort();
    let _ = output_task.await;

    relay_result
}

async fn relay_client_input(
    client_id: u64,
    runtime: Arc<SerialRuntime>,
    client_read: &mut tokio::net::unix::OwnedReadHalf,
) -> io::Result<()> {
    let mut buf = [0u8; 4096];

    loop {
        let n = client_read.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        let is_owner = runtime.hub.lock().await.can_write_input(client_id);
        if !is_owner {
            return Ok(());
        }

        let mut guest_input = runtime.guest_input.lock().await;
        guest_input.write_all(&buf[..n]).await?;
        guest_input.flush().await?;
    }
}

async fn wait_for_client_disconnect(
    client_read: &mut tokio::net::unix::OwnedReadHalf,
) -> io::Result<()> {
    let mut buf = [0u8; 256];
    loop {
        let n = client_read.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
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
