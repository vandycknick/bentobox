use chrono::Utc;
use eyre::Context;
use serde::{Deserialize, Serialize};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::flag;
use std::io::Read;
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
        ControlRequest, ControlResponse, CONTROL_OP_OPEN_VSOCK, CONTROL_PROTOCOL_VERSION,
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
    // listener
    //     .set_nonblocking(true)
    //     .context("set control socket nonblocking")?;

    Ok(SocketGuard {
        path: path.to_path_buf(),
        listener,
    })
}

fn handle_client(mut stream: UnixStream, driver: &dyn crate::driver::Driver) -> eyre::Result<()> {
    let request = read_control_request(&mut stream)?;

    if request.version != CONTROL_PROTOCOL_VERSION {
        let response = ControlResponse::error(
            request.id,
            "unsupported_version",
            format!(
                "control protocol version {} is unsupported",
                request.version
            ),
        );
        write_control_response(&mut stream, &response)?;
        return Ok(());
    }

    if request.op != CONTROL_OP_OPEN_VSOCK {
        let response = ControlResponse::error(request.id, "unsupported_op", "unsupported op");
        write_control_response(&mut stream, &response)?;
        return Ok(());
    }

    let Some(port) = request.port else {
        let response = ControlResponse::error(request.id, "missing_port", "port is required");
        write_control_response(&mut stream, &response)?;
        return Ok(());
    };

    let id = request.id;
    match driver.open_vsock_stream(port) {
        Ok(vsock_fd) => {
            write_control_response(&mut stream, &ControlResponse::ok(id))?;
            spawn_tunnel(stream, vsock_fd);
        }
        Err(err) => {
            let response = ControlResponse::error(
                id,
                "guest_port_unreachable",
                format!("failed to open guest vsock port {port}: {err}"),
            );
            write_control_response(&mut stream, &response)?;
        }
    }

    Ok(())
}

fn read_control_request(stream: &mut UnixStream) -> eyre::Result<ControlRequest> {
    let line = read_json_line(stream).context("read control request")?;
    if line.is_empty() {
        return Err(eyre::eyre!("control request stream closed before request"));
    }

    let req =
        serde_json::from_str::<ControlRequest>(&line).context("parse control request json")?;
    Ok(req)
}

fn read_json_line(stream: &mut UnixStream) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let n = stream.read(&mut byte)?;
        if n == 0 {
            break;
        }

        if byte[0] == b'\n' {
            break;
        }

        buf.push(byte[0]);
        if buf.len() > 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control line exceeded 16KiB",
            ));
        }
    }

    String::from_utf8(buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control line was not utf-8"))
}

fn write_control_response(stream: &mut UnixStream, response: &ControlResponse) -> eyre::Result<()> {
    let mut payload = serde_json::to_vec(response).context("serialize control response")?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .context("write control response")?;
    stream.flush().context("flush control response")?;
    Ok(())
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
