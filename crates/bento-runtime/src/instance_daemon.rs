use chrono::Utc;
use eyre::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::flag;
use std::collections::{BTreeMap, HashMap};
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::Duration;
use std::{fs, io};
use std::{fs::OpenOptions, io::Write, path::Path};

use crate::driver::{self, OpenDeviceRequest, OpenDeviceResponse};
use crate::{
    instance::InstanceFile,
    instance_control::{
        ControlErrorCode, ControlRequest, ControlRequestBody, ControlResponse, ServiceDescriptor,
        CONTROL_PROTOCOL_VERSION, SERVICE_SERIAL, SERVICE_SSH,
    },
    instance_manager::{Daemon, InstanceManager},
};

struct NopDaemon {}

impl Daemon for NopDaemon {
    fn stdin<T: Into<std::process::Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn stdout<T: Into<std::process::Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn stderr<T: Into<std::process::Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn spawn(&mut self) -> std::io::Result<std::process::Child> {
        std::process::Command::new("true").spawn()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InstancedEventType {
    Running,
    Exiting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstancedEvent {
    pub timestamp: String,

    #[serde(rename = "type")]
    pub event_type: InstancedEventType,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub struct InstanceDaemon {
    name: String,
    manager: InstanceManager<NopDaemon>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SerialAccess {
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
struct SerialRuntime {
    hub: Arc<Mutex<SerialHub>>,
    guest_input: Arc<Mutex<std::fs::File>>,
}

impl InstanceDaemon {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            manager: InstanceManager::new(NopDaemon {}),
        }
    }

    fn emit_event(&self, event: &InstancedEvent) -> eyre::Result<()> {
        let mut out = io::stdout().lock();
        let mut data = serde_json::to_vec(event).context("serialize instanced event")?;
        data.push(b'\n');
        out.write_all(&data).context("write instanced event")?;
        out.flush().context("flush instanced event")?;
        Ok(())
    }

    pub fn run(&self) -> eyre::Result<()> {
        let inst = self.manager.inspect(&self.name)?;

        // NOTE: PidFileGuard will auto drop the id.pid file at the end of the run function
        let _pid_file_guard =
            write_pid_file(&inst.file(InstanceFile::InstancedPid), std::process::id())?;
        let socket = bind_socket(&inst.file(InstanceFile::InstancedSocket))?;

        let mut driver = driver::get_driver_for(&inst)?;

        driver.start()?;

        let serial_runtime = match create_serial_runtime(&inst, &*driver) {
            Ok(runtime) => runtime,
            Err(err) => {
                let _ = driver.stop();
                return Err(err);
            }
        };

        self.emit_event(&InstancedEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type: InstancedEventType::Running,
            message: None,
        })?;

        let terminated = Arc::new(AtomicBool::new(false));
        flag::register(SIGINT, terminated.clone()).context("register SIGINT")?;
        flag::register(SIGTERM, terminated.clone()).context("register SIGTERM")?;

        while !terminated.load(Ordering::Relaxed) {
            match socket.listener.accept() {
                Ok((stream, _)) => {
                    if let Err(err) = handle_client(stream, &*driver, serial_runtime.clone()) {
                        eprintln!("[instanced] shell control request failed: {err}");
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    eprintln!("[instanced] control socket accept error: {err}");
                    thread::sleep(Duration::from_millis(250));
                }
            }
        }

        driver.stop()?;

        Ok(())
    }
}

#[must_use = "hold this guard for the process lifetime to keep control socket cleanup active"]
pub struct SocketGuard {
    path: PathBuf,
    listener: UnixListener,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn bind_socket(path: &Path) -> eyre::Result<SocketGuard> {
    if let Err(err) = fs::remove_file(path) {
        if err.kind() != io::ErrorKind::NotFound {
            return Err(err).context(format!("remove stale socket {}", path.display()));
        }
    }

    let listener = UnixListener::bind(path).context(format!("bind socket {}", path.display()))?;
    listener
        .set_nonblocking(true)
        .context("set control socket nonblocking")?;

    Ok(SocketGuard {
        path: path.to_path_buf(),
        listener,
    })
}

fn handle_client(
    mut stream: UnixStream,
    driver: &dyn crate::driver::Driver,
    serial_runtime: Arc<SerialRuntime>,
) -> eyre::Result<()> {
    let request = ControlRequest::read_from(&mut stream).context("read control request")?;
    let service_registry = ServiceRegistry::default();

    if request.version != CONTROL_PROTOCOL_VERSION {
        let response = ControlResponse::v1_error(
            request.id,
            ControlErrorCode::UnsupportedVersion,
            format!(
                "control protocol version {} is unsupported",
                request.version
            ),
        );
        response
            .write_to(&mut stream)
            .context("write control response")?;
        return Ok(());
    }

    match request.body {
        ControlRequestBody::ListServices => {
            let response = ControlResponse::v1_services(request.id, service_registry.describe());
            response
                .write_to(&mut stream)
                .context("write control response")?;
        }
        ControlRequestBody::OpenService { service, options } => {
            let id = request.id;
            let target = match service_registry.resolve(&service) {
                Some(target) => target,
                None => {
                    let response = ControlResponse::v1_error(
                        id,
                        ControlErrorCode::UnknownService,
                        format!("service '{service}' is not registered"),
                    );
                    response
                        .write_to(&mut stream)
                        .context("write control response")?;
                    return Ok(());
                }
            };

            match target {
                ServiceTarget::VsockPort(port) => {
                    if !options.is_empty() {
                        ControlResponse::v1_error(
                            id,
                            ControlErrorCode::UnsupportedRequest,
                            "ssh service does not accept options",
                        )
                        .write_to(&mut stream)
                        .context("write control response")?;
                        return Ok(());
                    }

                    let mut last_err = None;

                    for attempt in 1..=SERVICE_OPEN_MAX_ATTEMPTS {
                        match driver.open_device(OpenDeviceRequest::Vsock { port }) {
                            Ok(OpenDeviceResponse::Vsock { stream: vsock_fd }) => {
                                ControlResponse::v1_opened(id.clone())
                                    .write_to(&mut stream)
                                    .context("write control response")?;
                                spawn_tunnel(stream, vsock_fd);
                                return Ok(());
                            }
                            Ok(_) => {
                                last_err = Some(eyre::eyre!(
                                    "driver returned unexpected device type for ssh service"
                                ));
                            }
                            Err(err) => {
                                last_err = Some(eyre::eyre!(err));

                                if attempt < SERVICE_OPEN_MAX_ATTEMPTS {
                                    ControlResponse::v1_starting(
                                        id.clone(),
                                        attempt,
                                        SERVICE_OPEN_MAX_ATTEMPTS,
                                        SERVICE_OPEN_RETRY_DELAY_SECS,
                                    )
                                    .write_to(&mut stream)
                                    .context("write control response")?;

                                    thread::sleep(Duration::from_secs(
                                        SERVICE_OPEN_RETRY_DELAY_SECS,
                                    ));
                                }
                            }
                        }
                    }

                    let err_text = last_err
                        .map(|err| err.to_string())
                        .unwrap_or_else(|| "unknown startup error".to_string());

                    ControlResponse::v1_error(
                        id,
                        ControlErrorCode::ServiceUnavailable,
                        format!(
                            "failed to open service '{service}' on vsock port {port} after {} attempts: {err_text}",
                            SERVICE_OPEN_MAX_ATTEMPTS
                        ),
                    )
                    .write_to(&mut stream)
                    .context("write control response")?;
                }
                ServiceTarget::Serial => {
                    let serial_options = match parse_serial_open_options(options) {
                        Ok(options) => options,
                        Err(message) => {
                            ControlResponse::v1_error(
                                id,
                                ControlErrorCode::UnsupportedRequest,
                                message,
                            )
                            .write_to(&mut stream)
                            .context("write control response")?;
                            return Ok(());
                        }
                    };

                    let access = serial_options.access;
                    ControlResponse::v1_opened(id.clone())
                        .write_to(&mut stream)
                        .context("write control response")?;
                    spawn_serial_tunnel(stream, serial_runtime.clone(), access);
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ServiceTarget {
    VsockPort(u32),
    Serial,
}

#[derive(Debug)]
struct ServiceRegistry {
    by_name: BTreeMap<String, ServiceTarget>,
}

impl ServiceRegistry {
    fn resolve(&self, name: &str) -> Option<ServiceTarget> {
        self.by_name.get(name).copied()
    }

    fn describe(&self) -> Vec<ServiceDescriptor> {
        self.by_name
            .keys()
            .map(|name| ServiceDescriptor { name: name.clone() })
            .collect()
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        let mut by_name = BTreeMap::new();
        by_name.insert(SERVICE_SSH.to_string(), ServiceTarget::VsockPort(2222));
        by_name.insert(SERVICE_SERIAL.to_string(), ServiceTarget::Serial);

        Self { by_name }
    }
}

const SERVICE_OPEN_MAX_ATTEMPTS: u8 = 5;
const SERVICE_OPEN_RETRY_DELAY_SECS: u64 = 2;

fn parse_serial_open_options(options: Map<String, Value>) -> Result<SerialOpenOptions, String> {
    serde_json::from_value::<SerialOpenOptions>(Value::Object(options))
        .map_err(|err| format!("invalid serial options: {err}"))
}

fn create_serial_runtime(
    inst: &crate::instance::Instance,
    driver: &dyn crate::driver::Driver,
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
            let n = match std::io::Read::read(&mut guest_output, &mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    eprintln!("[instanced] serial read failed: {err}");
                    break;
                }
            };

            let chunk = &buf[..n];
            if let Err(err) = serial_log.write_all(chunk) {
                eprintln!("[instanced] serial log write failed: {err}");
            }
            let _ = serial_log.flush();

            match hub.lock() {
                Ok(mut hub) => hub.broadcast(chunk),
                Err(err) => {
                    eprintln!("[instanced] serial hub lock poisoned: {err}");
                    break;
                }
            }
        }
    });
}

fn spawn_serial_tunnel(stream: UnixStream, runtime: Arc<SerialRuntime>, access: SerialAccess) {
    thread::spawn(move || {
        if let Err(err) = proxy_serial_stream(stream, runtime, access) {
            eprintln!("[instanced] serial relay failed: {err}");
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
                let n = std::io::Read::read(&mut client_stream, &mut buf)?;
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

fn spawn_tunnel(stream: UnixStream, vsock_fd: OwnedFd) {
    thread::spawn(move || {
        if let Err(err) = proxy_streams(stream, vsock_fd) {
            eprintln!("[instanced] vsock relay failed: {err}");
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

#[must_use = "hold this guard for the process lifetime to keep PID file cleanup active"]
pub struct PidFileGuard {
    path: PathBuf,
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn write_pid_file(path: &Path, pid: u32) -> eyre::Result<PidFileGuard> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .context(format!("open {}", path.display()))?;

    writeln!(file, "{pid}").context("write pid")?;
    file.flush().context("flush pid")?;
    Ok(PidFileGuard {
        path: path.to_path_buf(),
    })
}
