use std::io;
use std::path::Path;
use std::sync::Arc;

use bento_machine::{MachineHandle, SerialStream as MachineSerialStream};
use eyre::Context;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, Mutex};

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
struct SerialAttachment {
    guest_input: WriteHalf<MachineSerialStream>,
    reader_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub(crate) struct SerialConsole {
    machine: MachineHandle,
    hub: Arc<Mutex<SerialHub>>,
    attachment: Arc<Mutex<Option<SerialAttachment>>>,
    file_sinks: Arc<Mutex<Vec<tokio::fs::File>>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    attach_lock: Arc<Mutex<()>>,
}

#[derive(Debug)]
pub(crate) struct SerialStream {
    console: Arc<SerialConsole>,
    client_id: u64,
    access: SerialAccess,
    output_rx: broadcast::Receiver<Vec<u8>>,
}

impl SerialConsole {
    pub(crate) fn new(machine: MachineHandle) -> Arc<Self> {
        let (output_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            machine,
            hub: Arc::new(Mutex::new(SerialHub::new())),
            attachment: Arc::new(Mutex::new(None)),
            file_sinks: Arc::new(Mutex::new(Vec::new())),
            output_tx,
            attach_lock: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) async fn stream_to_file(&self, path: &Path) -> eyre::Result<()> {
        self.ensure_attached().await?;

        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("open {}", path.display()))?;

        self.file_sinks.lock().await.push(file);
        Ok(())
    }

    pub(crate) async fn open_stream(
        self: &Arc<Self>,
        access: SerialAccess,
    ) -> eyre::Result<SerialStream> {
        self.ensure_attached().await?;

        let client_id = {
            let mut hub = self.hub.lock().await;
            hub.attach(access)?
        };

        Ok(SerialStream {
            console: self.clone(),
            client_id,
            access,
            output_rx: self.output_tx.subscribe(),
        })
    }

    async fn ensure_attached(&self) -> eyre::Result<()> {
        if self.attachment.lock().await.is_some() {
            return Ok(());
        }

        let _guard = self.attach_lock.lock().await;
        if self.attachment.lock().await.is_some() {
            return Ok(());
        }

        let stream = self.open_serial_device().await?;
        let (guest_output, guest_input) = tokio::io::split(stream);
        let output_tx = self.output_tx.clone();
        let file_sinks = self.file_sinks.clone();
        let reader_task = tokio::spawn(async move {
            run_serial_reader(guest_output, file_sinks, output_tx).await;
        });

        let attachment = SerialAttachment {
            guest_input,
            reader_task,
        };

        *self.attachment.lock().await = Some(attachment);
        Ok(())
    }

    async fn open_serial_device(&self) -> eyre::Result<MachineSerialStream> {
        self.machine
            .open_serial()
            .await
            .context("open serial device")
    }

    async fn write_input(&self, client_id: u64, chunk: &[u8]) -> io::Result<()> {
        let is_owner = self.hub.lock().await.can_write_input(client_id);
        if !is_owner {
            return Ok(());
        }

        let mut attachment = self.attachment.lock().await;
        let Some(attachment) = attachment.as_mut() else {
            return Err(io::Error::other("serial console is not attached"));
        };

        attachment.guest_input.write_all(chunk).await?;
        attachment.guest_input.flush().await
    }

    async fn detach(&self, client_id: u64) {
        let mut hub = self.hub.lock().await;
        hub.detach(client_id);
    }
}

impl SerialStream {
    async fn write_input(&self, chunk: &[u8]) -> io::Result<()> {
        match self.access {
            SerialAccess::Interactive => self.console.write_input(self.client_id, chunk).await,
            SerialAccess::Watch => Ok(()),
        }
    }
}

impl Drop for SerialStream {
    fn drop(&mut self) {
        let console = self.console.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            console.detach(client_id).await;
        });
    }
}

impl Drop for SerialConsole {
    fn drop(&mut self) {
        if let Ok(mut attachment) = self.attachment.try_lock() {
            if let Some(attachment) = attachment.take() {
                attachment.reader_task.abort();
            }
        }
    }
}

async fn run_serial_reader(
    mut guest_output: ReadHalf<MachineSerialStream>,
    file_sinks: Arc<Mutex<Vec<tokio::fs::File>>>,
    output_tx: broadcast::Sender<Vec<u8>>,
) {
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

        let chunk = buf[..n].to_vec();

        {
            let mut sinks = file_sinks.lock().await;
            let mut index = 0;
            while index < sinks.len() {
                let file = &mut sinks[index];
                match file.write_all(&chunk).await {
                    Ok(()) => {
                        let _ = file.flush().await;
                        index += 1;
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "serial log write failed");
                        sinks.remove(index);
                    }
                }
            }
        }

        let _ = output_tx.send(chunk);
    }
}

pub(crate) fn spawn_serial_tunnel(stream: UnixStream, serial_stream: SerialStream) {
    tokio::spawn(async move {
        if let Err(err) = proxy_serial_stream(stream, serial_stream).await {
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
    mut serial_stream: SerialStream,
) -> io::Result<()> {
    let access = serial_stream.access;
    let (mut client_read, mut client_write) = client_stream.into_split();
    let mut output_rx = std::mem::replace(
        &mut serial_stream.output_rx,
        serial_stream.console.output_tx.subscribe(),
    );

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
        SerialAccess::Interactive => relay_client_input(&mut serial_stream, &mut client_read).await,
        SerialAccess::Watch => wait_for_client_disconnect(&mut client_read).await,
    };

    output_task.abort();
    let _ = output_task.await;

    relay_result
}

async fn relay_client_input(
    serial_stream: &mut SerialStream,
    client_read: &mut tokio::net::unix::OwnedReadHalf,
) -> io::Result<()> {
    let mut buf = [0u8; 4096];

    loop {
        let n = client_read.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }

        serial_stream.write_input(&buf[..n]).await?;
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
