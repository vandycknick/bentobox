use bento_runtime::driver::{Driver, OpenDeviceRequest, OpenDeviceResponse};
use bento_runtime::instance::InstanceFile;
use eyre::Context;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SerialAccess {
    Interactive,
    Watch,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialOpenOptions {
    #[serde(default = "default_serial_access")]
    access: SerialAccess,
}

fn default_serial_access() -> SerialAccess {
    SerialAccess::Interactive
}

#[derive(Debug)]
struct SerialHub {
    next_id: u64,
    interactive_owner: Option<u64>,
    subscribers: HashMap<u64, mpsc::SyncSender<Vec<u8>>>,
}

impl SerialHub {
    fn new() -> Self {
        Self {
            next_id: 1,
            interactive_owner: None,
            subscribers: HashMap::new(),
        }
    }

    fn attach(&mut self, access: SerialAccess) -> eyre::Result<(u64, mpsc::Receiver<Vec<u8>>)> {
        if access == SerialAccess::Interactive && self.interactive_owner.is_some() {
            eyre::bail!("interactive serial client is already attached");
        }

        let id = self.next_id;
        self.next_id += 1;

        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(64);
        self.subscribers.insert(id, tx);
        if access == SerialAccess::Interactive {
            self.interactive_owner = Some(id);
        }

        Ok((id, rx))
    }

    fn detach(&mut self, id: u64) {
        self.subscribers.remove(&id);
        if self.interactive_owner == Some(id) {
            self.interactive_owner = None;
        }
    }

    fn can_write_input(&self, id: u64) -> bool {
        self.interactive_owner == Some(id)
    }

    fn broadcast(&mut self, data: &[u8]) {
        let payload = data.to_vec();
        let mut disconnected = Vec::new();

        for (id, tx) in &self.subscribers {
            match tx.try_send(payload.clone()) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(_)) | Err(mpsc::TrySendError::Disconnected(_)) => {
                    disconnected.push(*id)
                }
            }
        }

        for id in disconnected {
            self.detach(id);
        }
    }
}

#[derive(Debug)]
pub(crate) struct SerialRuntime {
    hub: Arc<Mutex<SerialHub>>,
    guest_input: Arc<Mutex<std::fs::File>>,
}

pub(crate) fn parse_serial_open_options(
    options: Map<String, Value>,
) -> Result<SerialAccess, String> {
    serde_json::from_value::<SerialOpenOptions>(Value::Object(options))
        .map(|options| options.access)
        .map_err(|err| format!("invalid serial options: {err}"))
}

pub(crate) fn create_serial_runtime(
    inst: &bento_runtime::instance::Instance,
    driver: &dyn Driver,
) -> eyre::Result<Arc<SerialRuntime>> {
    let device = driver
        .open_device(OpenDeviceRequest::Serial)
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

    let serial_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(inst.file(InstanceFile::SerialLog))
        .context("open serial.log")?;

    let runtime = Arc::new(SerialRuntime {
        hub: Arc::new(Mutex::new(SerialHub::new())),
        guest_input: Arc::new(Mutex::new(std::fs::File::from(guest_input))),
    });

    spawn_serial_reader(
        std::fs::File::from(guest_output),
        serial_log,
        runtime.hub.clone(),
    );

    Ok(runtime)
}

fn spawn_serial_reader(
    mut guest_output: std::fs::File,
    mut serial_log: std::fs::File,
    hub: Arc<Mutex<SerialHub>>,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];

        loop {
            let n = match Read::read(&mut guest_output, &mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    tracing::error!(error = %err, "serial read failed");
                    break;
                }
            };

            let chunk = &buf[..n];
            if let Err(err) = serial_log.write_all(chunk) {
                tracing::error!(error = %err, "serial log write failed");
            }
            let _ = serial_log.flush();

            match hub.lock() {
                Ok(mut hub) => hub.broadcast(chunk),
                Err(err) => {
                    tracing::error!(error = %err, "serial hub lock poisoned");
                    break;
                }
            }
        }
    });
}

pub(crate) fn spawn_serial_tunnel(
    stream: UnixStream,
    runtime: Arc<SerialRuntime>,
    access: SerialAccess,
) {
    thread::spawn(move || {
        if let Err(err) = proxy_serial_stream(stream, runtime, access) {
            tracing::error!(error = %err, "serial relay failed");
        }
    });
}

fn proxy_serial_stream(
    mut client_stream: UnixStream,
    runtime: Arc<SerialRuntime>,
    access: SerialAccess,
) -> io::Result<()> {
    client_stream.set_nonblocking(false)?;
    let (client_id, output_rx) = {
        let mut hub = runtime
            .hub
            .lock()
            .map_err(|_| io::Error::other("serial hub mutex poisoned"))?;
        hub.attach(access)
            .map_err(|err| io::Error::other(format!("{err}")))?
    };

    let mut output_stream = client_stream.try_clone()?;
    let output_task = thread::spawn(move || -> io::Result<()> {
        while let Ok(chunk) = output_rx.recv() {
            output_stream.write_all(&chunk)?;
            output_stream.flush()?;
        }
        Ok(())
    });

    if access == SerialAccess::Interactive {
        let runtime_input = runtime.clone();
        let input_task = thread::spawn(move || -> io::Result<()> {
            let mut buf = [0u8; 4096];
            loop {
                let n = Read::read(&mut client_stream, &mut buf)?;
                if n == 0 {
                    break;
                }

                let is_owner = runtime_input
                    .hub
                    .lock()
                    .map_err(|_| io::Error::other("serial hub mutex poisoned"))?
                    .can_write_input(client_id);
                if !is_owner {
                    break;
                }

                let mut guest_input = runtime_input
                    .guest_input
                    .lock()
                    .map_err(|_| io::Error::other("serial input mutex poisoned"))?;
                guest_input.write_all(&buf[..n])?;
                guest_input.flush()?;
            }
            Ok(())
        });

        match input_task.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let mut hub = runtime
                    .hub
                    .lock()
                    .map_err(|_| io::Error::other("serial hub mutex poisoned"))?;
                hub.detach(client_id);
                return Err(err);
            }
            Err(_) => {
                let mut hub = runtime
                    .hub
                    .lock()
                    .map_err(|_| io::Error::other("serial hub mutex poisoned"))?;
                hub.detach(client_id);
                return Err(io::Error::other("serial input relay thread panicked"));
            }
        }
    }

    {
        let mut hub = runtime
            .hub
            .lock()
            .map_err(|_| io::Error::other("serial hub mutex poisoned"))?;
        hub.detach(client_id);
    }

    match output_task.join() {
        Ok(result) => result,
        Err(_) => Err(io::Error::other("serial output relay thread panicked")),
    }
}
