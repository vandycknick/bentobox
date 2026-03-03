use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use bento_runtime::driver;
use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_manager::Daemon;
use bento_runtime::instance_manager::InstanceManager;
use chrono::Utc;
use eyre::Context;
use nix::unistd::setsid;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;

use crate::control::handle_client;
use crate::events::{emit_event, InstancedEvent, InstancedEventType};
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
        driver.start()?;

        let serial_runtime = match create_serial_runtime(&inst, &*driver) {
            Ok(runtime) => runtime,
            Err(err) => {
                let _ = driver.stop();
                return Err(err);
            }
        };

        emit_event(&InstancedEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type: InstancedEventType::Running,
            message: None,
        })?;

        tracing::info!(instance = %self.name, "instanced running");

        let mut sigint = signal(SignalKind::interrupt()).context("register SIGINT handler")?;
        let mut sigterm = signal(SignalKind::terminate()).context("register SIGTERM handler")?;

        loop {
            tokio::select! {
                accepted = socket.listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let stream = stream.into_std().context("convert accepted stream")?;
                            stream
                                .set_nonblocking(false)
                                .context("set accepted control stream blocking")?;
                            let serial_runtime = serial_runtime.clone();
                            let driver_ref = &*driver;
                            let result = handle_client(stream, driver_ref, serial_runtime).await;
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
                    break;
                }
                _ = sigterm.recv() => {
                    tracing::info!(instance = %self.name, "received SIGTERM, shutting down instanced");
                    break;
                }
            }
        }

        tracing::info!(instance = %self.name, "sending stop signal to vm");

        driver.stop()?;

        tracing::info!(instance = %self.name, "instance stopped");

        Ok(())
    }
}
