use std::sync::Arc;
use std::time::Duration;

use bento_runtime::driver;
use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_manager::InstanceManager;
use eyre::Context;
use futures::future::{FutureExt, LocalBoxFuture};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;

use crate::control::handle_client;
use crate::discovery::ServiceRegistry;
use crate::instance_control_service::InstanceControlState;
use crate::launcher::NoopLauncher;
use crate::pid_guard::PidGuard;
use crate::serial::create_serial_runtime;
use crate::socket::bind_socket;

pub struct InstanceDaemon {
    name: String,
    manager: InstanceManager<NoopLauncher>,
}

impl InstanceDaemon {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            manager: InstanceManager::new(NoopLauncher),
        }
    }

    pub async fn run(&self) -> eyre::Result<()> {
        let inst = self.manager.inspect(&self.name)?;
        let _trace_guard = init_tracing(&inst.file(InstanceFile::InstancedTraceLog))?;
        tracing::info!(instance = %self.name, "instanced starting");

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

        let expects_guest_agent = inst.expects_guest_agent();
        if expects_guest_agent {
            control_state.publish_guest_state(
                bento_protocol::instance::v1::LifecycleState::Starting,
                "waiting for guest services",
            );
        }

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
        let mut guest_probe_tick = tokio::time::interval(Duration::from_secs(1));
        let mut guest_ready = !expects_guest_agent;
        let mut client_tasks = FuturesUnordered::<LocalBoxFuture<'_, ()>>::new();

        loop {
            tokio::select! {
                accepted = socket.listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let serial_runtime = serial_runtime.clone();
                            let control_state = control_state.clone();
                            let driver_ref = &*driver;
                            client_tasks.push(
                                async move {
                                    let result = handle_client(
                                        stream,
                                        driver_ref,
                                        serial_runtime,
                                        control_state,
                                    )
                                    .await;
                                    if let Err(err) = result {
                                        tracing::warn!(error = %err, "shell control request failed");
                                    }
                                }
                                .boxed_local(),
                            );
                        }
                        Err(err) => {
                            tracing::error!(error = %err, "control socket accept error");
                        }
                    }
                }
                _ = guest_probe_tick.tick(), if !guest_ready && expects_guest_agent => {
                    match ServiceRegistry::discover(&*driver).await {
                        Ok(_) => {
                            guest_ready = true;
                            control_state.publish_guest_state(
                                bento_protocol::instance::v1::LifecycleState::Running,
                                "guest services ready",
                            );
                        }
                        Err(err) => {
                            tracing::info!(error = %err, "guest services not ready yet");
                        }
                    }
                }
                Some(_) = client_tasks.next(), if !client_tasks.is_empty() => {}
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

        drop(client_tasks);

        driver.stop()?;

        control_state.publish_vm_state(
            bento_protocol::instance::v1::LifecycleState::Stopped,
            "vm stopped",
        );

        tracing::info!(instance = %self.name, "instance stopped");

        Ok(())
    }
}

fn init_tracing(
    trace_path: &std::path::Path,
) -> eyre::Result<tracing_appender::non_blocking::WorkerGuard> {
    let trace_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace_path)
        .context(format!("open {}", trace_path.display()))?;

    let (writer, guard) = tracing_appender::non_blocking(trace_file);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .with_writer(writer)
        .try_init()
        .map_err(|err| eyre::eyre!("initialize instanced tracing: {err}"))?;

    Ok(guard)
}
