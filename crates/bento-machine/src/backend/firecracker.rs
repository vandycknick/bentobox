use crate::backend::MachineBackend;
use crate::stream::{RawSerialConnection, RawVsockConnection};
use crate::types::{
    MachineError, MachineExitEvent, MachineExitReceiver, MachineKind, MachineState, NetworkMode,
    ResolvedMachineSpec,
};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use reqwest::blocking::Client;
use reqwest::Method;
use serde::Serialize;
use std::env;
use std::fs::{self, File};
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const FIRECRACKER_BINARY_ENV: &str = "FIRECRACKER_BIN";
const FIRECRACKER_BINARY_NAME: &str = "firecracker";
const API_SOCKET_NAME: &str = "firecracker.sock";
const TRACE_LOG_NAME: &str = "fc.trace.log";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct FirecrackerMachineBackend {
    spec: ResolvedMachineSpec,
    firecracker_bin: PathBuf,
    runtime_dir: PathBuf,
    api_socket_path: PathBuf,
    trace_log_path: PathBuf,
    api_client: FirecrackerApiClient,
    state: Arc<Mutex<MachineState>>,
    exit_sender: Option<Arc<Mutex<Option<oneshot::Sender<MachineExitEvent>>>>>,
    running: Option<RunningFirecracker>,
}

struct RunningFirecracker {
    child: Arc<Mutex<Child>>,
    serial_read: File,
    serial_write: File,
    exit_watcher: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for FirecrackerMachineBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrackerMachineBackend")
            .field("id", &self.spec.id.as_str())
            .field("runtime_dir", &self.runtime_dir)
            .finish_non_exhaustive()
    }
}

impl FirecrackerMachineBackend {
    pub(crate) fn new(spec: ResolvedMachineSpec) -> Result<Self, MachineError> {
        validate(&spec)?;
        let firecracker_bin = locate_firecracker_binary()?;
        let runtime_dir = runtime_dir_for(&spec);
        let api_socket_path = runtime_dir.join(API_SOCKET_NAME);
        let trace_log_path = runtime_dir.join(TRACE_LOG_NAME);
        let api_client = FirecrackerApiClient::new(api_socket_path.clone())?;

        Ok(Self {
            spec,
            firecracker_bin,
            runtime_dir,
            api_socket_path,
            trace_log_path,
            api_client,
            state: Arc::new(Mutex::new(MachineState::Created)),
            exit_sender: None,
            running: None,
        })
    }

    fn set_state(&self, state: MachineState) -> Result<(), MachineError> {
        let mut current = self
            .state
            .lock()
            .map_err(|_| MachineError::RegistryPoisoned)?;
        *current = state;
        Ok(())
    }

    fn ensure_runtime_dir(&self) -> Result<(), MachineError> {
        fs::create_dir_all(&self.runtime_dir)?;
        if self.api_socket_path.exists() {
            fs::remove_file(&self.api_socket_path)?;
        }
        Ok(())
    }

    fn refresh_state_from_child(&self) -> Result<(), MachineError> {
        let Some(running) = self.running.as_ref() else {
            return Ok(());
        };

        let mut child = running
            .child
            .lock()
            .map_err(|_| MachineError::RegistryPoisoned)?;
        let status = try_wait_child(&mut child)?;

        if let Some(status) = status {
            self.set_state(MachineState::Stopped)?;
            send_exit_once(
                self.exit_sender.as_ref(),
                MachineState::Stopped,
                &format_exit_message(status),
            );
        }

        Ok(())
    }

    fn serial_connection(&self) -> Result<RawSerialConnection, MachineError> {
        self.refresh_state_from_child()?;

        if self.state()? != MachineState::Running {
            return Err(MachineError::Backend(format!(
                "cannot open serial stream because machine {:?} is not running",
                self.spec.id.as_str()
            )));
        }

        let running = self.running.as_ref().ok_or_else(|| {
            MachineError::Backend(format!(
                "cannot open serial stream because machine {:?} is not running",
                self.spec.id.as_str()
            ))
        })?;

        Ok(RawSerialConnection {
            read: running.serial_read.try_clone()?,
            write: running.serial_write.try_clone()?,
        })
    }

    fn wait_for_api_socket(
        &mut self,
        child: &mut Child,
        timeout_duration: Duration,
    ) -> Result<(), MachineError> {
        tracing::debug!(
            api_socket = %self.api_socket_path.display(),
            timeout_ms = timeout_duration.as_millis(),
            "waiting for firecracker API socket"
        );
        let deadline = Instant::now() + timeout_duration;
        loop {
            if self.api_socket_path.exists() && self.api_client.ping().is_ok() {
                return Ok(());
            }

            if let Some(status) = try_wait_child(child)? {
                return Err(MachineError::Backend(format!(
                    "firecracker exited before the API socket became ready: {}",
                    format_exit_message(status)
                )));
            }

            if Instant::now() >= deadline {
                return Err(MachineError::Backend(format!(
                    "timed out waiting for firecracker API socket at {}",
                    self.api_socket_path.display()
                )));
            }

            thread::sleep(EXIT_POLL_INTERVAL);
        }
    }

    fn configure_and_start_vm(&mut self) -> Result<(), MachineError> {
        let config = &self.spec.config;
        tracing::debug!(
            machine_id = self.spec.id.as_str(),
            "sending firecracker machine configuration"
        );
        self.api_client.put_json(
            "/machine-config",
            &MachineConfigurationRequest {
                vcpu_count: config.cpus.expect("validated cpus missing"),
                mem_size_mib: config.memory_mib.expect("validated memory missing"),
                smt: false,
                track_dirty_pages: false,
            },
        )?;
        tracing::debug!(
            machine_id = self.spec.id.as_str(),
            "sending firecracker boot source configuration"
        );
        self.api_client.put_json(
            "/boot-source",
            &BootSourceRequest {
                kernel_image_path: config
                    .kernel_path
                    .as_ref()
                    .expect("validated kernel path missing")
                    .display()
                    .to_string(),
                initrd_path: config
                    .initramfs_path
                    .as_ref()
                    .expect("validated initramfs path missing")
                    .display()
                    .to_string(),
                boot_args: default_boot_args().to_string(),
            },
        )?;
        tracing::debug!(
            machine_id = self.spec.id.as_str(),
            "sending firecracker instance start action"
        );
        self.api_client.put_json(
            "/actions",
            &ActionRequest {
                action_type: "InstanceStart",
            },
        )
    }
}

pub(crate) fn validate(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    let config = &spec.config;
    if config.cpus.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires a CPU count".to_string(),
        });
    }

    if config.memory_mib.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires a memory size".to_string(),
        });
    }

    if config.kernel_path.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires a kernel image path".to_string(),
        });
    }

    if config.initramfs_path.is_none() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires an initramfs path".to_string(),
        });
    }

    if matches!(config.cpus, Some(0)) {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires at least one vCPU".to_string(),
        });
    }

    if matches!(config.memory_mib, Some(0)) {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "firecracker requires memory_mib to be greater than zero".to_string(),
        });
    }

    if config.root_disk.is_some() {
        return not_implemented(
            spec,
            "root disks are not implemented for the firecracker backend yet",
        );
    }

    if !config.data_disks.is_empty() {
        return not_implemented(
            spec,
            "data disks are not implemented for the firecracker backend yet",
        );
    }

    if !config.mounts.is_empty() {
        return not_implemented(
            spec,
            "shared directory mounts are not implemented for the firecracker backend yet",
        );
    }

    if config.machine_identifier_path.is_some() {
        return not_implemented(
            spec,
            "machine identifiers are not used by the firecracker backend",
        );
    }

    if config.nested_virtualization {
        return not_implemented(
            spec,
            "nested virtualization is not implemented for the firecracker backend yet",
        );
    }

    if config.rosetta {
        return not_implemented(
            spec,
            "rosetta is not implemented for the firecracker backend",
        );
    }

    match config.network {
        NetworkMode::None => {}
        NetworkMode::VzNat => {
            return not_implemented(spec, "vznat networking is only supported by the VZ backend");
        }
        NetworkMode::Bridged => {
            return not_implemented(
                spec,
                "bridged networking is not implemented for the firecracker backend yet",
            );
        }
        NetworkMode::Cni => {
            return not_implemented(
                spec,
                "cni networking is not implemented for the firecracker backend yet",
            );
        }
    }

    Ok(())
}

pub(crate) fn prepare(spec: &ResolvedMachineSpec) -> Result<(), MachineError> {
    validate(spec)?;
    validate_support()?;

    let kernel_path = spec
        .config
        .kernel_path
        .as_ref()
        .expect("validated kernel path missing");
    let initramfs_path = spec
        .config
        .initramfs_path
        .as_ref()
        .expect("validated initramfs path missing");

    if spec.config.machine_directory.as_os_str().is_empty() {
        return Err(MachineError::InvalidConfig {
            id: spec.id.clone(),
            reason: "machine_directory must be set".to_string(),
        });
    }

    ensure_path_exists(spec, kernel_path, "kernel image")?;
    ensure_path_exists(spec, initramfs_path, "initramfs")?;

    fs::create_dir_all(runtime_dir_for(spec))?;
    Ok(())
}

impl MachineBackend for FirecrackerMachineBackend {
    fn state(&self) -> Result<MachineState, MachineError> {
        self.refresh_state_from_child()?;
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)
    }

    fn start(&mut self) -> Result<MachineExitReceiver, MachineError> {
        if self.state()? == MachineState::Running {
            return Err(MachineError::AlreadyRunning {
                id: self.spec.id.clone(),
            });
        }

        if self.running.is_some() {
            return Err(MachineError::Backend(
                "firecracker restart is not implemented yet".to_string(),
            ));
        }

        prepare(&self.spec)?;
        self.ensure_runtime_dir()?;

        tracing::info!(
            machine_id = self.spec.id.as_str(),
            firecracker_bin = %self.firecracker_bin.display(),
            api_socket = %self.api_socket_path.display(),
            trace_log = %self.trace_log_path.display(),
            kernel = %self.spec.config.kernel_path.as_ref().expect("validated kernel path missing").display(),
            initramfs = %self.spec.config.initramfs_path.as_ref().expect("validated initramfs path missing").display(),
            cpus = self.spec.config.cpus.expect("validated cpus missing"),
            memory_mib = self.spec.config.memory_mib.expect("validated memory missing"),
            "starting firecracker backend"
        );

        if self.trace_log_path.exists() {
            fs::remove_file(&self.trace_log_path)?;
        }

        let mut child = Command::new(&self.firecracker_bin)
            .arg("--api-sock")
            .arg(&self.api_socket_path)
            .arg("--log-path")
            .arg(&self.trace_log_path)
            .arg("--level")
            .arg("Info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        tracing::info!(
            machine_id = self.spec.id.as_str(),
            pid = child.id(),
            "spawned firecracker process"
        );

        if let Err(err) = self.wait_for_api_socket(&mut child, STARTUP_TIMEOUT) {
            let _ = terminate_child(&mut child);
            return Err(err);
        }

        tracing::info!(
            machine_id = self.spec.id.as_str(),
            api_socket = %self.api_socket_path.display(),
            "firecracker API socket is ready"
        );

        if let Err(err) = self.configure_and_start_vm() {
            let _ = terminate_child(&mut child);
            return Err(err);
        }

        tracing::info!(
            machine_id = self.spec.id.as_str(),
            "firecracker VM start request completed"
        );

        let serial_write = File::from(OwnedFd::from(child.stdin.take().ok_or_else(|| {
            MachineError::Backend("firecracker child stdin pipe was not available".to_string())
        })?));
        let serial_read = File::from(OwnedFd::from(child.stdout.take().ok_or_else(|| {
            MachineError::Backend("firecracker child stdout pipe was not available".to_string())
        })?));

        let (exit_tx, exit_rx) = oneshot::channel();
        let shared_exit = Arc::new(Mutex::new(Some(exit_tx)));
        let shared_child = Arc::new(Mutex::new(child));
        let state = self.state.clone();
        let exit_watcher = spawn_exit_watcher(
            self.spec.id.as_str().to_string(),
            shared_child.clone(),
            state,
            shared_exit.clone(),
        );

        self.running = Some(RunningFirecracker {
            child: shared_child,
            serial_read,
            serial_write,
            exit_watcher: Some(exit_watcher),
        });
        self.set_state(MachineState::Running)?;
        self.exit_sender = Some(shared_exit);
        Ok(exit_rx)
    }

    fn stop(&mut self) -> Result<(), MachineError> {
        let running = match self.running.take() {
            Some(running) => running,
            None => {
                self.set_state(MachineState::Stopped)?;
                return Ok(());
            }
        };

        {
            let mut child = running
                .child
                .lock()
                .map_err(|_| MachineError::RegistryPoisoned)?;

            if try_wait_child(&mut child)?.is_none() {
                let pid = child.id();
                tracing::info!(
                    machine_id = self.spec.id.as_str(),
                    pid,
                    "sending SIGTERM to firecracker process"
                );
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);

                let deadline = Instant::now() + STOP_TIMEOUT;
                loop {
                    if try_wait_child(&mut child)?.is_some() {
                        break;
                    }

                    if Instant::now() >= deadline {
                        tracing::warn!(
                            machine_id = self.spec.id.as_str(),
                            pid,
                            "firecracker did not stop after SIGTERM, sending SIGKILL"
                        );
                        child.kill()?;
                        let _ = child.wait()?;
                        break;
                    }

                    thread::sleep(EXIT_POLL_INTERVAL);
                }
            }
        }

        if let Some(exit_watcher) = running.exit_watcher {
            let _ = exit_watcher.join();
        }

        send_exit_once(
            self.exit_sender.as_ref(),
            MachineState::Stopped,
            "machine stopped by host request",
        );
        self.exit_sender = None;
        self.set_state(MachineState::Stopped)?;
        if self.api_socket_path.exists() {
            let _ = fs::remove_file(&self.api_socket_path);
        }
        Ok(())
    }

    fn open_vsock(&self, _port: u32) -> Result<RawVsockConnection, MachineError> {
        Err(MachineError::Unimplemented {
            kind: MachineKind::Firecracker,
            operation: "open_vsock",
        })
    }

    fn open_serial(&self) -> Result<RawSerialConnection, MachineError> {
        self.serial_connection()
    }
}

fn validate_support() -> Result<(), MachineError> {
    locate_firecracker_binary()?;
    if !Path::new("/dev/kvm").exists() {
        return Err(MachineError::UnsupportedBackend {
            kind: MachineKind::Firecracker,
            reason: "firecracker requires /dev/kvm on Linux hosts".to_string(),
        });
    }

    Ok(())
}

fn locate_firecracker_binary() -> Result<PathBuf, MachineError> {
    if let Some(path) = env::var_os(FIRECRACKER_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }

        return Err(MachineError::UnsupportedBackend {
            kind: MachineKind::Firecracker,
            reason: format!(
                "{FIRECRACKER_BINARY_ENV} is set but does not point to a file: {}",
                path.display()
            ),
        });
    }

    let path = env::var_os("PATH").ok_or_else(|| MachineError::UnsupportedBackend {
        kind: MachineKind::Firecracker,
        reason: "PATH is not set, so the firecracker binary cannot be located".to_string(),
    })?;

    for entry in env::split_paths(&path) {
        let candidate = entry.join(FIRECRACKER_BINARY_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(MachineError::UnsupportedBackend {
        kind: MachineKind::Firecracker,
        reason: "firecracker binary was not found in PATH".to_string(),
    })
}

fn runtime_dir_for(spec: &ResolvedMachineSpec) -> PathBuf {
    spec.config.machine_directory.clone()
}

fn ensure_path_exists(
    spec: &ResolvedMachineSpec,
    path: &Path,
    label: &str,
) -> Result<(), MachineError> {
    if path.exists() {
        return Ok(());
    }

    Err(MachineError::InvalidConfig {
        id: spec.id.clone(),
        reason: format!("{label} does not exist: {}", path.display()),
    })
}

fn not_implemented(spec: &ResolvedMachineSpec, reason: &str) -> Result<(), MachineError> {
    Err(MachineError::InvalidConfig {
        id: spec.id.clone(),
        reason: reason.to_string(),
    })
}

fn default_boot_args() -> &'static str {
    "console=ttyS0 reboot=k panic=1 pci=off"
}

fn terminate_child(child: &mut Child) -> Result<(), MachineError> {
    if try_wait_child(child)?.is_some() {
        return Ok(());
    }

    let pid = child.id();
    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        if try_wait_child(child)?.is_some() {
            return Ok(());
        }

        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait()?;
            return Ok(());
        }

        thread::sleep(EXIT_POLL_INTERVAL);
    }
}

fn spawn_exit_watcher(
    machine_id: String,
    child: Arc<Mutex<Child>>,
    state: Arc<Mutex<MachineState>>,
    exit_sender: Arc<Mutex<Option<oneshot::Sender<MachineExitEvent>>>>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("firecracker-machine-state:{machine_id}"))
        .spawn(move || loop {
            let status = match child.lock() {
                Ok(mut child) => try_wait_child(&mut child),
                Err(_) => return,
            };

            match status {
                Ok(Some(status)) => {
                    if let Ok(mut current_state) = state.lock() {
                        *current_state = MachineState::Stopped;
                    }
                    tracing::info!(machine_id, status = %format_exit_message(status), "firecracker process exited");
                    let _ = send_exit_once_inner(
                        &exit_sender,
                        MachineState::Stopped,
                        &format_exit_message(status),
                    );
                    return;
                }
                Ok(None) => thread::sleep(EXIT_POLL_INTERVAL),
                Err(err) => {
                    if let Ok(mut current_state) = state.lock() {
                        *current_state = MachineState::Stopped;
                    }
                    tracing::warn!(machine_id, error = %err, "failed to poll firecracker process status");
                    let _ = send_exit_once_inner(
                        &exit_sender,
                        MachineState::Stopped,
                        &format!("failed to poll firecracker process status: {err}"),
                    );
                    return;
                }
            }
        })
        .expect("firecracker exit watcher thread should spawn")
}

fn send_exit_once(
    exit_sender: Option<&Arc<Mutex<Option<oneshot::Sender<MachineExitEvent>>>>>,
    state: MachineState,
    message: &str,
) {
    let Some(exit_sender) = exit_sender else {
        return;
    };
    let _ = send_exit_once_inner(exit_sender, state, message);
}

fn send_exit_once_inner(
    exit_sender: &Arc<Mutex<Option<oneshot::Sender<MachineExitEvent>>>>,
    state: MachineState,
    message: &str,
) -> Result<(), MachineError> {
    let sender = exit_sender
        .lock()
        .map_err(|_| MachineError::RegistryPoisoned)?
        .take();

    if let Some(sender) = sender {
        let _ = sender.send(MachineExitEvent {
            state,
            message: message.to_string(),
        });
    }

    Ok(())
}

fn format_exit_message(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("firecracker exited with status code {code}"),
        None => "firecracker exited due to signal".to_string(),
    }
}

fn try_wait_child(child: &mut Child) -> Result<Option<ExitStatus>, MachineError> {
    match child.try_wait() {
        Ok(status) => Ok(status),
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(err) => Err(MachineError::Io(err)),
    }
}

struct FirecrackerApiClient {
    socket_path: PathBuf,
    client: Client,
}

impl FirecrackerApiClient {
    fn new(socket_path: PathBuf) -> Result<Self, MachineError> {
        let client = Client::builder()
            .http1_only()
            .connect_timeout(STARTUP_TIMEOUT)
            .timeout(STARTUP_TIMEOUT)
            .unix_socket(socket_path.clone())
            .build()
            .map_err(|err| {
                MachineError::Backend(format!(
                    "build firecracker reqwest client failed for {}: {err}",
                    socket_path.display()
                ))
            })?;
        Ok(Self {
            socket_path,
            client,
        })
    }

    fn put_json<T: Serialize>(&self, path: &str, body: &T) -> Result<(), MachineError> {
        self.send_request(Method::PUT, path, Some(body))
    }

    fn ping(&self) -> Result<(), MachineError> {
        self.send_request::<()>(Method::GET, "/", None)
    }

    fn send_request<T: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&T>,
    ) -> Result<(), MachineError> {
        tracing::debug!(method = %method, path, api_socket = %self.socket_path.display(), "sending firecracker API request");
        let url = format!("http://localhost{path}");
        let request = self
            .client
            .request(method.clone(), &url)
            .header("Accept", "application/json");

        let request = if let Some(body) = body {
            request
                .header("Content-Type", "application/json")
                .json(body)
        } else {
            request
        };

        let response = request.send().map_err(|err| {
            MachineError::Backend(format!(
                "firecracker API request failed for {} {}: {err}",
                method, path
            ))
        })?;

        let status = response.status();
        let response_text = response.text().map_err(|err| {
            MachineError::Backend(format!(
                "read firecracker API response failed for {} {}: {err}",
                method, path
            ))
        })?;

        if status.is_success() {
            tracing::debug!(method = %method, path, status_code = status.as_u16(), "firecracker API request succeeded");
            return Ok(());
        }

        tracing::warn!(
            method = %method,
            path,
            status_code = status.as_u16(),
            response_body = response_text.trim(),
            "firecracker API request failed"
        );
        Err(MachineError::Backend(format!(
            "firecracker API request {} {} failed with status {}: {}",
            method,
            path,
            status,
            response_text.trim()
        )))
    }
}

#[derive(Serialize)]
struct MachineConfigurationRequest {
    vcpu_count: usize,
    mem_size_mib: u64,
    smt: bool,
    track_dirty_pages: bool,
}

#[derive(Serialize)]
struct BootSourceRequest {
    kernel_image_path: String,
    initrd_path: String,
    boot_args: String,
}

#[derive(Serialize)]
struct ActionRequest {
    action_type: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MachineConfig, MachineId, MachineSpec};

    fn spec() -> ResolvedMachineSpec {
        MachineSpec {
            id: MachineId::from("firecracker-test"),
            kind: Some(MachineKind::Firecracker),
            config: MachineConfig {
                cpus: Some(2),
                memory_mib: Some(256),
                machine_directory: PathBuf::from("/tmp/firecracker-test"),
                kernel_path: Some(PathBuf::from("/tmp/kernel")),
                initramfs_path: Some(PathBuf::from("/tmp/initramfs")),
                network: NetworkMode::None,
                ..MachineConfig::new()
            },
        }
        .resolve()
        .expect("spec should resolve")
    }

    #[test]
    fn default_boot_args_enable_serial_console() {
        assert!(default_boot_args().contains("console=ttyS0"));
    }

    #[test]
    fn validate_rejects_root_disk() {
        let mut spec = spec();
        spec.config.root_disk = Some(crate::types::DiskImage {
            path: PathBuf::from("/tmp/rootfs"),
            read_only: false,
        });

        let err = validate(&spec).expect_err("root disk should be rejected");
        assert!(matches!(err, MachineError::InvalidConfig { .. }));
    }

    #[test]
    fn validate_requires_initramfs() {
        let mut spec = spec();
        spec.config.initramfs_path = None;

        let err = validate(&spec).expect_err("initramfs should be required");
        assert!(matches!(err, MachineError::InvalidConfig { .. }));
    }
}
