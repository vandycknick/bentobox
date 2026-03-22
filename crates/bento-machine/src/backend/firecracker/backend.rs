use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use tokio::sync::{watch, Mutex as AsyncMutex};

use crate::backend::firecracker::api::{
    ActionRequest, BootSourceRequest, DriveRequest, FirecrackerApiClient,
    MachineConfigurationRequest, VsockRequest,
};
use crate::backend::firecracker::config::{
    build_boot_args, guest_cid_for, locate_firecracker_binary, prepare, runtime_dir_for,
    API_SOCKET_NAME, TRACE_LOG_NAME, VSOCK_SOCKET_NAME,
};
use crate::backend::firecracker::process::{
    format_exit_message, spawn_exit_watcher, terminate_child, try_wait_child, FirecrackerRuntime,
    RunningFirecracker, EXIT_POLL_INTERVAL, STARTUP_TIMEOUT, STOP_TIMEOUT,
};
use crate::stream::{RawSerialConnection, RawVsockConnection};
use crate::types::{MachineError, MachineState, MachineStateReceiver, ResolvedMachineSpec};

pub(crate) struct FirecrackerMachineBackend {
    spec: ResolvedMachineSpec,
    firecracker_bin: PathBuf,
    runtime_dir: PathBuf,
    api_socket_path: PathBuf,
    trace_log_path: PathBuf,
    vsock_socket_path: PathBuf,
    api_client: FirecrackerApiClient,
    state: Arc<Mutex<MachineState>>,
    runtime: AsyncMutex<FirecrackerRuntime>,
    state_tx: watch::Sender<MachineState>,
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
        super::validate(&spec)?;
        let firecracker_bin = locate_firecracker_binary()?;
        let runtime_dir = runtime_dir_for(&spec);
        let api_socket_path = runtime_dir.join(API_SOCKET_NAME);
        let trace_log_path = runtime_dir.join(TRACE_LOG_NAME);
        let vsock_socket_path = runtime_dir.join(VSOCK_SOCKET_NAME);
        let api_client = FirecrackerApiClient::new(api_socket_path.clone(), STARTUP_TIMEOUT)?;
        let (state_tx, _state_rx) = watch::channel(MachineState::Created);

        Ok(Self {
            spec,
            firecracker_bin,
            runtime_dir,
            api_socket_path,
            trace_log_path,
            vsock_socket_path,
            api_client,
            state: Arc::new(Mutex::new(MachineState::Created)),
            runtime: AsyncMutex::new(FirecrackerRuntime::default()),
            state_tx,
        })
    }

    pub(crate) async fn state(&self) -> Result<MachineState, MachineError> {
        let mut runtime = self.runtime.lock().await;
        self.refresh_state_from_child_locked(&mut runtime)?;
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)
    }

    pub(crate) async fn start(&self) -> Result<(), MachineError> {
        let mut runtime = self.runtime.lock().await;
        self.refresh_state_from_child_locked(&mut runtime)?;
        if self
            .state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)?
            == MachineState::Running
        {
            return Err(MachineError::AlreadyRunning {
                id: self.spec.id.clone(),
            });
        }

        if runtime.running.is_some() {
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

        let mut child = self.spawn_child()?;

        tracing::info!(
            machine_id = self.spec.id.as_str(),
            pid = child.id(),
            "spawned firecracker process"
        );

        if let Err(err) = self.wait_for_api_socket(&mut child) {
            let _ = terminate_child(&mut child);
            return Err(err);
        }

        tracing::info!(machine_id = self.spec.id.as_str(), api_socket = %self.api_socket_path.display(), "firecracker API socket is ready");

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

        let shared_child = Arc::new(Mutex::new(child));
        let state = self.state.clone();
        let exit_watcher = spawn_exit_watcher(
            self.spec.id.as_str().to_string(),
            shared_child.clone(),
            state,
            self.state_tx.clone(),
        );

        runtime.running = Some(RunningFirecracker {
            child: shared_child,
            serial_read,
            serial_write,
            exit_watcher: Some(exit_watcher),
        });
        self.set_state(MachineState::Running)?;
        let _ = self.state_tx.send(MachineState::Running);
        Ok(())
    }

    pub(crate) async fn stop(&self) -> Result<(), MachineError> {
        let mut runtime = self.runtime.lock().await;
        let running = match runtime.running.take() {
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

        self.set_state(MachineState::Stopped)?;
        let _ = self.state_tx.send(MachineState::Stopped);
        if self.api_socket_path.exists() {
            let _ = fs::remove_file(&self.api_socket_path);
        }
        if self.vsock_socket_path.exists() {
            let _ = fs::remove_file(&self.vsock_socket_path);
        }
        Ok(())
    }

    pub(crate) async fn open_vsock(&self, port: u32) -> Result<RawVsockConnection, MachineError> {
        let mut runtime = self.runtime.lock().await;
        self.refresh_state_from_child_locked(&mut runtime)?;
        if self
            .state
            .lock()
            .map(|state| *state)
            .map_err(|_| MachineError::RegistryPoisoned)?
            != MachineState::Running
        {
            return Err(MachineError::Backend(format!(
                "cannot open vsock stream because machine {:?} is not running",
                self.spec.id.as_str()
            )));
        }

        let mut stream = StdUnixStream::connect(&self.vsock_socket_path)?;
        stream.write_all(format!("CONNECT {port}\n").as_bytes())?;
        stream.flush()?;

        let mut reader = BufReader::new(stream.try_clone()?);
        let mut response = String::new();
        reader.read_line(&mut response)?;
        let expected_prefix = "OK ";
        if !response.starts_with(expected_prefix) {
            return Err(MachineError::Backend(format!(
                "firecracker vsock connection handshake failed for port {port}: {}",
                response.trim()
            )));
        }

        RawVsockConnection::from_unix(stream).map_err(MachineError::from)
    }

    pub(crate) async fn open_serial(&self) -> Result<RawSerialConnection, MachineError> {
        let mut runtime = self.runtime.lock().await;
        self.serial_connection_locked(&mut runtime)
    }

    pub(crate) fn subscribe_state(&self) -> MachineStateReceiver {
        self.state_tx.subscribe()
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
        if self.vsock_socket_path.exists() {
            let _ = fs::remove_file(&self.vsock_socket_path);
        }
        Ok(())
    }

    fn refresh_state_from_child_locked(
        &self,
        runtime: &mut FirecrackerRuntime,
    ) -> Result<(), MachineError> {
        let Some(running) = runtime.running.as_ref() else {
            return Ok(());
        };

        let mut child = running
            .child
            .lock()
            .map_err(|_| MachineError::RegistryPoisoned)?;
        let status = try_wait_child(&mut child)?;

        if let Some(status) = status {
            self.set_state(MachineState::Stopped)?;
            let _ = self.state_tx.send(MachineState::Stopped);
        }

        Ok(())
    }

    fn serial_connection_locked(
        &self,
        runtime: &mut FirecrackerRuntime,
    ) -> Result<RawSerialConnection, MachineError> {
        self.refresh_state_from_child_locked(runtime)?;

        if self.state()? != MachineState::Running {
            return Err(MachineError::Backend(format!(
                "cannot open serial stream because machine {:?} is not running",
                self.spec.id.as_str()
            )));
        }

        let running = runtime.running.as_ref().ok_or_else(|| {
            MachineError::Backend(format!(
                "cannot open serial stream because machine {:?} is not running",
                self.spec.id.as_str()
            ))
        })?;

        RawSerialConnection::from_files(
            running.serial_read.try_clone()?,
            running.serial_write.try_clone()?,
        )
        .map_err(MachineError::from)
    }

    fn wait_for_api_socket(&self, child: &mut Child) -> Result<(), MachineError> {
        tracing::debug!(
            api_socket = %self.api_socket_path.display(),
            timeout_ms = STARTUP_TIMEOUT.as_millis(),
            "waiting for firecracker API socket"
        );
        let deadline = Instant::now() + STARTUP_TIMEOUT;
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

    fn configure_and_start_vm(&self) -> Result<(), MachineError> {
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
                boot_args: build_boot_args(config),
            },
        )?;
        for (index, disk, is_root_device) in config
            .root_disk
            .iter()
            .map(|disk| (0usize, disk, true))
            .chain(
                config
                    .data_disks
                    .iter()
                    .enumerate()
                    .map(|(index, disk)| (index + 1, disk, false)),
            )
        {
            let drive_id = if index == 0 {
                "rootfs".to_string()
            } else {
                format!("disk{index}")
            };
            self.api_client.put_json(
                &format!("/drives/{drive_id}"),
                &DriveRequest {
                    drive_id,
                    partuuid: None,
                    is_root_device: index == 0 && is_root_device,
                    cache_type: "Writeback",
                    is_read_only: disk.read_only,
                    path_on_host: disk.path.display().to_string(),
                    io_engine: "Sync",
                },
            )?;
        }
        self.api_client.put_json(
            "/vsock",
            &VsockRequest {
                guest_cid: guest_cid_for(&self.spec),
                uds_path: self.vsock_socket_path.display().to_string(),
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

    fn spawn_child(&self) -> Result<Child, MachineError> {
        Command::new(&self.firecracker_bin)
            .arg("--api-sock")
            .arg(&self.api_socket_path)
            .arg("--log-path")
            .arg(&self.trace_log_path)
            .arg("--level")
            .arg("Info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(MachineError::from)
    }
}
