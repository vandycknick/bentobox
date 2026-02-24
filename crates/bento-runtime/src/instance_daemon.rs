use chrono::Utc;
use eyre::Context;
use serde::{Deserialize, Serialize};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::flag;
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;
use std::{fs, io};
use std::{fs::OpenOptions, io::Write, path::Path};

use crate::driver::{self};
use crate::{
    instance::InstanceFile,
    instance_control::{
        ControlErrorCode, ControlRequest, ControlRequestBody, ControlResponse, ServiceDescriptor,
        CONTROL_PROTOCOL_VERSION, SERVICE_SSH,
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
                    if let Err(err) = handle_client(stream, &*driver) {
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

fn handle_client(mut stream: UnixStream, driver: &dyn crate::driver::Driver) -> eyre::Result<()> {
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
        ControlRequestBody::OpenService { service } => {
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
                    let mut last_err = None;

                    for attempt in 1..=SERVICE_OPEN_MAX_ATTEMPTS {
                        match driver.open_vsock_stream(port) {
                            Ok(vsock_fd) => {
                                ControlResponse::v1_opened(id.clone())
                                    .write_to(&mut stream)
                                    .context("write control response")?;
                                spawn_tunnel(stream, vsock_fd);
                                return Ok(());
                            }
                            Err(err) => {
                                last_err = Some(err);

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
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ServiceTarget {
    VsockPort(u32),
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

        Self { by_name }
    }
}

const SERVICE_OPEN_MAX_ATTEMPTS: u8 = 5;
const SERVICE_OPEN_RETRY_DELAY_SECS: u64 = 2;

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
