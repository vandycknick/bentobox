use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use bento_runtime::driver;
use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_manager::Daemon;
use bento_runtime::instance_manager::InstanceManager;
use eyre::Context;
use nix::unistd::setsid;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;

use crate::control::handle_client;
use crate::instance_control_service::InstanceControlState;
use crate::pid_guard::PidGuard;
use crate::serial::create_serial_runtime;
use crate::socket::bind_socket;

pub struct NixDaemon {
    command: Command,
}

impl NixDaemon {
    pub fn new(exe: impl AsRef<OsStr>) -> Self {
        let mut command = Command::new(exe.as_ref());
        unsafe {
            command.pre_exec(|| {
                setsid()
                    .map(|_| ())
                    .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
            });
        }

        Self { command }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.command.arg(arg);
        self
    }
}

impl Daemon for NixDaemon {
    fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdin(cfg);
        self
    }

    fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdout(cfg);
        self
    }

    fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stderr(cfg);
        self
    }

    fn spawn(&mut self) -> io::Result<Child> {
        self.command.spawn()
    }
}

pub(crate) struct NopDaemon;

impl Daemon for NopDaemon {
    fn stdin<T: Into<Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn stdout<T: Into<Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn stderr<T: Into<Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn spawn(&mut self) -> io::Result<Child> {
        Command::new("true").spawn()
    }
}

pub struct InstanceDaemon {
    name: String,
    manager: InstanceManager<NopDaemon>,
}

impl InstanceDaemon {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            manager: InstanceManager::new(NopDaemon),
        }
    }

    pub async fn run(&self) -> eyre::Result<()> {
        tracing::info!(instance = %self.name, "instanced starting");

        let inst = self.manager.inspect(&self.name)?;

        let _pid_guard = PidGuard::create(&inst.file(InstanceFile::InstancedPid)).await?;
        let socket = bind_socket(&inst.file(InstanceFile::InstancedSocket))?;

        let mut driver = driver::get_driver_for(&inst)?;
        let control_state = Arc::new(InstanceControlState::new());
        control_state.publish_vm_state(
            bento_protocol::instance::v1::LifecycleState::Starting,
            "vm starting",
        );

        driver.start()?;
        control_state.publish_vm_state(
            bento_protocol::instance::v1::LifecycleState::Running,
            "vm running",
        );
        control_state.publish_guest_state(
            bento_protocol::instance::v1::LifecycleState::Running,
            "mock guest ready",
        );

        let serial_runtime = match create_serial_runtime(&inst, &*driver) {
            Ok(runtime) => runtime,
            Err(err) => {
                let _ = driver.stop();
                return Err(err);
            }
        };

        tracing::info!(instance = %self.name, "instanced running");

        let mut sigint = signal(SignalKind::interrupt()).context("register SIGINT handler")?;
        let mut sigterm = signal(SignalKind::terminate()).context("register SIGTERM handler")?;

        loop {
            tokio::select! {
                accepted = socket.listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let serial_runtime = serial_runtime.clone();
                            let control_state = control_state.clone();
                            let driver_ref = &*driver;
                            let result =
                                handle_client(stream, driver_ref, serial_runtime, control_state)
                                    .await;
                            if let Err(err) = result {
                                tracing::warn!(error = %err, "shell control request failed");
                            }
                        }
                        Err(err) => {
                            tracing::error!(error = %err, "control socket accept error");
                        }
                    }
                }
                _ = sigint.recv() => {
                    tracing::info!(instance = %self.name, "received SIGINT, shutting down instanced");
                    control_state.publish_vm_state(
                        bento_protocol::instance::v1::LifecycleState::Stopping,
                        "received SIGINT",
                    );
                    break;
                }
                _ = sigterm.recv() => {
                    tracing::info!(instance = %self.name, "received SIGTERM, shutting down instanced");
                    control_state.publish_vm_state(
                        bento_protocol::instance::v1::LifecycleState::Stopping,
                        "received SIGTERM",
                    );
                    break;
                }
            }
        }

        tracing::info!(instance = %self.name, "sending stop signal to vm");

        driver.stop()?;

        control_state.publish_vm_state(
            bento_protocol::instance::v1::LifecycleState::Stopped,
            "vm stopped",
        );

        tracing::info!(instance = %self.name, "instance stopped");

        Ok(())
    }
}
