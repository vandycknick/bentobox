use bento_protocol::control::{
    ControlError, ControlErrorCode, ControlPlane, OpenServiceRequest, OpenServiceResponse,
    ServiceDescriptor, SERVICE_SERIAL, SERVICE_SSH,
};
use bento_protocol::guest::{
    GuestDiscoveryClient, HealthStatus, ServiceEndpoint, DEFAULT_DISCOVERY_PORT,
};
use chrono::Utc;
use eyre::Context;
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::thread;
use std::time::Duration;
use std::{fs, io};
use std::{fs::OpenOptions, io::Read, io::Write, path::Path};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UnixListener;

use crate::driver::{self, OpenDeviceRequest, OpenDeviceResponse};
use crate::{
    instance::InstanceFile,
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

    pub async fn run(&self) -> eyre::Result<()> {
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

        let driver = Rc::new(RefCell::new(driver));
        let control_service = ControlService {
            driver: driver.clone(),
            serial_runtime: serial_runtime.clone(),
            instance_dir: inst.dir().to_path_buf(),
        };

        self.emit_event(&InstancedEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type: InstancedEventType::Running,
            message: None,
        })?;

        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .context("register SIGINT handler")?;
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("register SIGTERM handler")?;

        tokio::task::LocalSet::new()
            .run_until(async move {
                loop {
                    tokio::select! {
                        result = socket.listener.accept() => {
                            match result {
                                Ok((stream, _)) => {
                                    let service = control_service.clone();
                                    tokio::task::spawn_local(async move {
                                        if let Err(err) = serve_control_client(stream, service).await {
                                            eprintln!("[instanced] shell control request failed: {err}");
                                        }
                                    });
                                }
                                Err(err) => {
                                    eprintln!("[instanced] control socket accept error: {err}");
                                    tokio::time::sleep(Duration::from_millis(250)).await;
                                }
                            }
                        }
                        _ = sigint.recv() => {
                            break;
                        }
                        _ = sigterm.recv() => {
                            break;
                        }
                    }
                }
            })
            .await;

        driver
            .borrow_mut()
            .stop()
            .map_err(|err| eyre::eyre!("driver stop failed: {err}"))?;

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

    Ok(SocketGuard {
        path: path.to_path_buf(),
        listener,
    })
}

async fn serve_control_client(
    stream: tokio::net::UnixStream,
    service: ControlService,
) -> eyre::Result<()> {
    use futures::StreamExt;
    use tarpc::serde_transport;
    use tarpc::server::{self, Channel};
    use tarpc::tokio_serde::formats::Bincode;
    use tarpc::tokio_util::codec::LengthDelimitedCodec;

    let framed = LengthDelimitedCodec::builder().new_framed(stream);
    let transport = serde_transport::new(framed, Bincode::default());

    server::BaseChannel::with_defaults(transport)
        .execute(service.serve())
        .for_each(|response| response)
        .await;

    Ok(())
}

#[derive(Clone)]
struct ControlService {
    driver: Rc<RefCell<Box<dyn crate::driver::Driver>>>,
    serial_runtime: Arc<SerialRuntime>,
    instance_dir: PathBuf,
}

impl ControlPlane for ControlService {
    async fn list_services(
        self,
        _: tarpc::context::Context,
    ) -> Result<Vec<ServiceDescriptor>, ControlError> {
        ServiceRegistry::discover(&self.driver)
            .await
            .map(|registry| registry.describe())
            .map_err(|err| {
                ControlError::new(
                    ControlErrorCode::ServiceUnavailable,
                    format!("discover guest services failed: {err}"),
                )
            })
    }

    async fn open_service(
        self,
        _: tarpc::context::Context,
        request: OpenServiceRequest,
    ) -> Result<OpenServiceResponse, ControlError> {
        let target = if request.service == SERVICE_SERIAL {
            Some(ServiceTarget::Serial)
        } else {
            let service_registry =
                ServiceRegistry::discover(&self.driver)
                    .await
                    .map_err(|err| {
                        ControlError::new(
                            ControlErrorCode::ServiceUnavailable,
                            format!("discover guest services failed: {err}"),
                        )
                    })?;
            service_registry.resolve(&request.service)
        };

        let Some(target) = target else {
            return Err(ControlError::new(
                ControlErrorCode::UnknownService,
                format!("service '{}' is not registered", request.service),
            ));
        };

        match target {
            ServiceTarget::VsockPort(port) => {
                if !request.options.is_empty() {
                    return Err(ControlError::new(
                        ControlErrorCode::UnsupportedRequest,
                        "ssh service does not accept options",
                    ));
                }

                let mut last_err = None;
                for _ in 1..=SERVICE_OPEN_MAX_ATTEMPTS {
                    let open_result = {
                        self.driver
                            .borrow()
                            .open_device(OpenDeviceRequest::Vsock { port })
                    };

                    match open_result {
                        Ok(OpenDeviceResponse::Vsock { stream: vsock_fd }) => {
                            let tunnel_socket =
                                bind_tunnel_socket(&self.instance_dir).map_err(|err| {
                                    ControlError::new(
                                        ControlErrorCode::Internal,
                                        format!("bind tunnel socket failed: {err}"),
                                    )
                                })?;
                            let socket_path = tunnel_socket.path.display().to_string();
                            tokio::task::spawn_local(async move {
                                if let Err(err) =
                                    accept_and_proxy_vsock_tunnel(tunnel_socket, vsock_fd).await
                                {
                                    eprintln!("[instanced] vsock relay failed: {err}");
                                }
                            });

                            return Ok(OpenServiceResponse { socket_path });
                        }
                        Ok(_) => {
                            last_err = Some(
                                "driver returned unexpected device type for ssh service"
                                    .to_string(),
                            );
                        }
                        Err(err) => {
                            last_err = Some(err.to_string());
                            tokio::time::sleep(Duration::from_secs(SERVICE_OPEN_RETRY_DELAY_SECS))
                                .await;
                        }
                    }
                }

                let err_text = last_err.unwrap_or_else(|| "unknown startup error".to_string());
                Err(ControlError::new(
                    ControlErrorCode::ServiceUnavailable,
                    format!(
                        "failed to open service '{}' on vsock port {} after {} attempts: {}",
                        request.service, port, SERVICE_OPEN_MAX_ATTEMPTS, err_text
                    ),
                ))
            }
            ServiceTarget::Serial => {
                let serial_options =
                    parse_serial_open_options(request.options).map_err(|message| {
                        ControlError::new(ControlErrorCode::UnsupportedRequest, message)
                    })?;

                let tunnel_socket = bind_tunnel_socket(&self.instance_dir).map_err(|err| {
                    ControlError::new(
                        ControlErrorCode::Internal,
                        format!("bind tunnel socket failed: {err}"),
                    )
                })?;
                let socket_path = tunnel_socket.path.display().to_string();
                let serial_runtime = self.serial_runtime.clone();
                tokio::task::spawn_local(async move {
                    if let Err(err) = accept_and_proxy_serial_tunnel(
                        tunnel_socket,
                        serial_runtime,
                        serial_options.access,
                    )
                    .await
                    {
                        eprintln!("[instanced] serial relay failed: {err}");
                    }
                });

                Ok(OpenServiceResponse { socket_path })
            }
        }
    }
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
    async fn discover(driver: &Rc<RefCell<Box<dyn crate::driver::Driver>>>) -> eyre::Result<Self> {
        let mut by_name = BTreeMap::new();
        by_name.insert(SERVICE_SERIAL.to_string(), ServiceTarget::Serial);

        let discovery_stream = {
            let driver_ref = driver.borrow();
            match driver_ref.open_device(OpenDeviceRequest::Vsock {
                port: DEFAULT_DISCOVERY_PORT,
            })? {
                OpenDeviceResponse::Vsock { stream } => stream,
                OpenDeviceResponse::Serial { .. } => {
                    eyre::bail!("driver returned serial device when opening guest discovery port")
                }
            }
        };

        for endpoint in fetch_guest_services_from_stream(discovery_stream).await? {
            by_name.insert(endpoint.name, ServiceTarget::VsockPort(endpoint.port));
        }

        Ok(Self { by_name })
    }

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

const SERVICE_OPEN_MAX_ATTEMPTS: u8 = 5;
const SERVICE_OPEN_RETRY_DELAY_SECS: u64 = 2;
const TUNNEL_ACCEPT_TIMEOUT_SECS: u64 = 15;

struct TunnelSocketGuard {
    path: PathBuf,
    listener: UnixListener,
}

impl Drop for TunnelSocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn bind_tunnel_socket(instance_dir: &Path) -> io::Result<TunnelSocketGuard> {
    let tunnel_dir = instance_dir.join("tunnels");
    fs::create_dir_all(&tunnel_dir)?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let path = tunnel_dir.join(format!("tunnel-{nonce}.sock"));
    let listener = UnixListener::bind(&path)?;

    Ok(TunnelSocketGuard { path, listener })
}

async fn accept_and_proxy_vsock_tunnel(
    tunnel_socket: TunnelSocketGuard,
    vsock_fd: OwnedFd,
) -> io::Result<()> {
    let (client, _) = tokio::time::timeout(
        Duration::from_secs(TUNNEL_ACCEPT_TIMEOUT_SECS),
        tunnel_socket.listener.accept(),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for tunnel client",
        )
    })??;

    proxy_streams_async(client, vsock_fd).await
}

async fn accept_and_proxy_serial_tunnel(
    tunnel_socket: TunnelSocketGuard,
    runtime: Arc<SerialRuntime>,
    access: SerialAccess,
) -> io::Result<()> {
    let (client, _) = tokio::time::timeout(
        Duration::from_secs(TUNNEL_ACCEPT_TIMEOUT_SECS),
        tunnel_socket.listener.accept(),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for tunnel client",
        )
    })??;

    let std_client = client.into_std()?;
    tokio::task::spawn_blocking(move || proxy_serial_stream(std_client, runtime, access))
        .await
        .map_err(|err| io::Error::other(format!("serial tunnel task join error: {err}")))?
}

async fn proxy_streams_async(
    mut client_stream: tokio::net::UnixStream,
    vsock_fd: OwnedFd,
) -> io::Result<()> {
    let mut vsock_stream = AsyncFdStream::new(std::fs::File::from(vsock_fd))?;
    let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut vsock_stream).await?;
    Ok(())
}

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

#[derive(Debug)]
struct AsyncFdStream {
    inner: AsyncFd<std::fs::File>,
}

impl AsyncFdStream {
    fn new(file: std::fs::File) -> io::Result<Self> {
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
        Poll::Ready(Ok(()))
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

async fn fetch_guest_services_from_stream(stream: OwnedFd) -> eyre::Result<Vec<ServiceEndpoint>> {
    use tarpc::context;
    use tarpc::serde_transport;
    use tarpc::tokio_serde::formats::Bincode;
    use tarpc::tokio_util::codec::LengthDelimitedCodec;

    let stream = AsyncFdStream::new(std::fs::File::from(stream))
        .context("wrap discovery stream in async fd")?;
    let framed = LengthDelimitedCodec::builder().new_framed(stream);
    let transport = serde_transport::new(framed, Bincode::default());
    let client = GuestDiscoveryClient::new(tarpc::client::Config::default(), transport).spawn();

    let HealthStatus { ok } =
        tokio::time::timeout(Duration::from_secs(3), client.health(context::current()))
            .await
            .map_err(|_| eyre::eyre!("guest discovery health request timed out"))?
            .map_err(|err| eyre::eyre!("query guest discovery health failed: {err}"))?;

    if !ok {
        eyre::bail!("guest discovery service reported unhealthy");
    }

    let endpoints = tokio::time::timeout(
        Duration::from_secs(3),
        client.list_services(context::current()),
    )
    .await
    .map_err(|_| eyre::eyre!("guest discovery list_services request timed out"))?
    .map_err(|err| eyre::eyre!("query guest service list failed: {err}"))?;

    if endpoints
        .iter()
        .all(|endpoint| endpoint.name != SERVICE_SSH)
    {
        eyre::bail!("guest discovery did not report ssh service");
    }

    Ok(endpoints)
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
