use std::sync::Arc;
use std::time::Duration;

use bento_machine::{Machine, MachineExitEvent, MachineState};
use bento_protocol::instance::v1::{ExtensionStatus, HostSocket, LifecycleState};
use bento_runtime::extensions::{BuiltinExtension, EXTENSION_DOCKER, EXTENSION_PORT_FORWARD};
use bento_runtime::instance::{Instance, InstanceFile};
use bento_runtime::instance_store::InstanceStore;
use bento_runtime::services::SERVICE_DOCKER;
use eyre::Context;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;

use crate::bootstrap::rebuild_bootstrap;
use crate::discovery::{request_guest_shutdown, ServiceRegistry};
use crate::host_export::listen_host_service;
use crate::machine::machine_spec_for_instance;
use crate::pid_guard::PidGuard;
use crate::port_forward::spawn_port_forward_manager;
use crate::serial::SerialConsole;
use crate::server::InstanceServer;
use crate::state::{new_instance_store, Action};

const GUEST_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const GUEST_DISCOVERY_RETRY: Duration = Duration::from_secs(1);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

enum LoopExit {
    Signal(&'static str),
    InternalExit(&'static str),
    VmStopped(MachineExitEvent),
}

pub struct InstanceDaemon {
    name: String,
    store: InstanceStore,
}

impl InstanceDaemon {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            store: InstanceStore::new(),
        }
    }

    pub async fn run(&self) -> eyre::Result<()> {
        let inst = self.store.inspect(&self.name)?;
        let _trace_guard = init_tracing(&inst.file(InstanceFile::InstancedTraceLog))?;
        tracing::info!(instance = %self.name, "instanced starting");

        rebuild_bootstrap(&inst)?;

        remove_stale_socket(&inst.file(InstanceFile::InstancedSocket))?;

        let machine_spec = machine_spec_for_instance(&inst)?;
        let machine = Machine::create(machine_spec.clone()).await?;
        let serial_console = SerialConsole::new(machine.clone());
        let store = Arc::new(new_instance_store());
        let expects_guest_agent = inst.uses_bootstrap();

        let server = InstanceServer::new(machine.clone(), serial_console.clone(), store.clone());
        let server_task = server.listen(&inst.file(InstanceFile::InstancedSocket))?;

        let docker_socket_path = docker_host_socket_path(&inst);
        remove_stale_socket(&docker_socket_path)?;
        let docker_export_task = if inst.extensions().is_enabled(BuiltinExtension::Docker) {
            store.dispatch(Action::set_host_sockets(vec![HostSocket {
                name: String::from(SERVICE_DOCKER),
                path: docker_socket_path.display().to_string(),
            }]));

            Some(listen_host_service(
                machine.clone(),
                &docker_socket_path,
                String::from(SERVICE_DOCKER),
            )?)
        } else {
            store.dispatch(Action::set_host_sockets(Vec::new()));
            None
        };

        let _pid_guard = PidGuard::create(&inst.file(InstanceFile::InstancedPid)).await?;

        let _port_forward_task = if inst.extensions().is_enabled(BuiltinExtension::PortForward) {
            Some(spawn_port_forward_manager_when_guest_ready(
                machine.clone(),
                store.clone(),
            ))
        } else {
            store.dispatch(Action::set_port_forwards(Vec::new()));
            None
        };

        store.dispatch(Action::vm_starting());

        let mut machine_exit = machine.start().await?;

        if let Err(err) = serial_console
            .stream_to_file(&inst.file(InstanceFile::SerialLog))
            .await
        {
            let _ = machine.stop().await;
            return Err(err);
        }

        store.dispatch(Action::vm_running());

        tracing::info!(instance = %self.name, "instanced running");

        let mut sigint = signal(SignalKind::interrupt()).context("register SIGINT handler")?;
        let mut sigterm = signal(SignalKind::terminate()).context("register SIGTERM handler")?;

        let mut guest_extension_monitor_task = if expects_guest_agent {
            store.dispatch(Action::guest_starting());
            Some(spawn_guest_extension_monitor(
                inst.clone(),
                machine.clone(),
                store.clone(),
            ))
        } else {
            store.dispatch(Action::set_extensions(Vec::new()));
            store.dispatch(Action::guest_running());
            None
        };

        let loop_exit = tokio::select! {
            _ = sigint.recv() => {
                tracing::info!(instance = %self.name, "received SIGINT, shutting down instanced");
                store.dispatch(Action::VmTransition {
                    state: bento_protocol::instance::v1::LifecycleState::Stopping,
                    message: String::from("received SIGINT"),
                });
                LoopExit::Signal("SIGINT")
            }
            _ = sigterm.recv() => {
                tracing::info!(instance = %self.name, "received SIGTERM, shutting down instanced");
                store.dispatch(Action::VmTransition {
                    state: bento_protocol::instance::v1::LifecycleState::Stopping,
                    message: String::from("received SIGTERM"),
                });
                LoopExit::Signal("SIGTERM")
            }
            result = &mut machine_exit => {
                match result {
                    Ok(event) => {
                        tracing::info!(instance = %self.name, state = ?event.state, message = %event.message, "machine exited");
                        LoopExit::VmStopped(event)
                    }
                    Err(_) => {
                        tracing::warn!(instance = %self.name, "machine exit channel closed");
                        LoopExit::VmStopped(MachineExitEvent {
                            state: MachineState::Stopped,
                            message: String::from("machine exit channel closed"),
                        })
                    }
                }
            }
            result = server_task => {
                match result {
                    Ok(Ok(())) => {
                        tracing::warn!(instance = %self.name, "instance server exited");
                    }
                    Ok(Err(err)) => return Err(err),
                    Err(err) => {
                        return Err(eyre::eyre!("instance server task failed: {err}"));
                    }
                }
                LoopExit::InternalExit("instance server exited")
            }
            result = async {
                if let Some(task) = docker_export_task {
                    task.await
                } else {
                    std::future::pending().await
                }
            } => {
                match result {
                    Ok(Ok(())) => tracing::warn!(instance = %self.name, "docker host export exited"),
                    Ok(Err(err)) => return Err(err),
                    Err(err) => return Err(eyre::eyre!("docker host export task failed: {err}")),
                }
                LoopExit::InternalExit("docker host export exited")
            }
        };

        if let Some(task) = guest_extension_monitor_task.take() {
            task.abort();
        }

        let mut vm_already_stopped = matches!(loop_exit, LoopExit::VmStopped(_));

        if let LoopExit::Signal(reason) = &loop_exit {
            tracing::info!(instance = %self.name, reason, "beginning shutdown flow");
            if expects_guest_agent {
                match request_guest_shutdown(&machine, false).await {
                    Ok(()) => {
                        tracing::info!(instance = %self.name, "requested graceful guest shutdown");
                        match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, &mut machine_exit)
                            .await
                        {
                            Ok(Ok(event)) => {
                                tracing::info!(instance = %self.name, state = ?event.state, message = %event.message, "guest shutdown observed before force-stop fallback");
                                vm_already_stopped = true;
                            }
                            Ok(Err(_)) => {
                                tracing::warn!(instance = %self.name, "machine exit channel closed during graceful shutdown wait");
                            }
                            Err(_) => {
                                tracing::warn!(instance = %self.name, timeout = ?GRACEFUL_SHUTDOWN_TIMEOUT, "graceful shutdown timed out, forcing stop");
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(instance = %self.name, error = %err, "failed to request graceful guest shutdown, forcing stop");
                    }
                }
            }
        } else if let LoopExit::InternalExit(reason) = &loop_exit {
            tracing::warn!(instance = %self.name, reason, "background task exited, forcing vm stop");
        }

        if let LoopExit::VmStopped(event) = &loop_exit {
            store.dispatch(Action::VmTransition {
                state: LifecycleState::Stopped,
                message: event.message.clone(),
            });
        }

        if vm_already_stopped {
            tracing::info!(instance = %self.name, "vm already stopped, finalizing instance shutdown");
        } else {
            tracing::info!(instance = %self.name, "sending stop signal to vm");
        }

        machine.stop().await?;

        store.dispatch(Action::VmTransition {
            state: LifecycleState::Stopped,
            message: String::from("vm stopped"),
        });

        let _ = std::fs::remove_file(&docker_socket_path);

        tracing::info!(instance = %self.name, "instance stopped");

        Ok(())
    }
}

fn spawn_guest_extension_monitor(
    inst: Instance,
    machine: bento_machine::MachineInstance,
    store: Arc<crate::state::InstanceStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + GUEST_DISCOVERY_TIMEOUT;
        let mut guest_probe_tick = tokio::time::interval(GUEST_DISCOVERY_RETRY);

        loop {
            guest_probe_tick.tick().await;

            match ServiceRegistry::discover(&machine).await {
                Ok(registry) => {
                    let extensions = project_extension_statuses(&inst, &registry);
                    let ready = startup_required_extensions_ready(&extensions);
                    let waiting_summary = startup_required_wait_summary(&extensions);
                    store.dispatch(Action::set_extensions(extensions));

                    if ready {
                        store.dispatch(Action::guest_running());
                    } else {
                        store.dispatch(Action::guest_starting());
                        tracing::info!(reason = %waiting_summary, "startup-required extensions not ready yet");
                    }
                }
                Err(err) if tokio::time::Instant::now() >= deadline => {
                    tracing::warn!(error = %err, timeout = ?GUEST_DISCOVERY_TIMEOUT, "guest extensions did not become ready before timeout");
                    store.dispatch(Action::guest_error(format!(
                        "guest discovery failed: {err}"
                    )));
                    return;
                }
                Err(err) => {
                    tracing::info!(reason = %classify_guest_discovery_retry(&err), "guest discovery not ready yet");
                    tracing::debug!(error = %err, "guest discovery retry detail");
                }
            }
        }
    })
}

fn project_extension_statuses(inst: &Instance, registry: &ServiceRegistry) -> Vec<ExtensionStatus> {
    inst.extensions()
        .enabled_extensions()
        .into_iter()
        .map(|extension| {
            let reported = registry
                .extensions()
                .find(|status| status.name == extension.id());
            let (configured, running, summary, problems) = match reported {
                Some(status) => (
                    status.configured,
                    status.running,
                    status.summary.clone(),
                    status.problems.clone(),
                ),
                None => (
                    false,
                    false,
                    format!("{} was not reported by guestd", extension.id()),
                    vec![format!("guestd did not report {}", extension.id())],
                ),
            };

            ExtensionStatus {
                name: extension.id().to_string(),
                enabled: true,
                startup_required: extension.startup_required(),
                configured,
                running,
                summary,
                problems,
            }
        })
        .collect()
}

fn startup_required_extensions_ready(extensions: &[ExtensionStatus]) -> bool {
    extensions
        .iter()
        .filter(|extension| extension.startup_required)
        .all(|extension| extension.configured && extension.running)
}

fn startup_required_wait_summary(extensions: &[ExtensionStatus]) -> String {
    let reasons = extensions
        .iter()
        .filter(|extension| extension.startup_required)
        .filter(|extension| !(extension.configured && extension.running))
        .map(|extension| {
            let detail = extension
                .problems
                .first()
                .cloned()
                .filter(|problem| !problem.is_empty())
                .unwrap_or_else(|| extension.summary.clone());
            format!("{}: {}", extension.name, detail)
        })
        .collect::<Vec<_>>();

    if reasons.is_empty() {
        String::from("waiting for startup-required extensions")
    } else {
        reasons.join("; ")
    }
}

fn classify_guest_discovery_retry(err: &eyre::Report) -> &'static str {
    let message = err.to_string().to_ascii_lowercase();

    if message.contains("unimplemented") {
        return "guestd protocol is older than instanced";
    }

    if message.contains("connection reset by peer")
        || message.contains("connection refused")
        || message.contains("not connected")
        || message.contains("service unavailable")
    {
        return "guestd is not reachable yet";
    }

    if message.contains("timed out") {
        return "guest discovery rpc timed out";
    }

    "waiting for guest discovery rpc"
}

fn spawn_port_forward_manager_when_guest_ready(
    machine: bento_machine::MachineInstance,
    store: Arc<crate::state::InstanceStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut guest_probe_tick = tokio::time::interval(GUEST_DISCOVERY_RETRY);

        loop {
            guest_probe_tick.tick().await;
            match ServiceRegistry::discover(&machine).await {
                Ok(_) => {
                    tracing::info!(
                        extension = EXTENSION_PORT_FORWARD,
                        "guest discovery is ready, starting port-forward manager"
                    );
                    break;
                }
                Err(err) => {
                    tracing::info!(
                        extension = EXTENSION_PORT_FORWARD,
                        reason = %classify_guest_discovery_retry(&err),
                        "port-forward waiting for guest discovery"
                    );
                    tracing::debug!(
                        extension = EXTENSION_PORT_FORWARD,
                        error = %err,
                        "port-forward guest discovery retry detail"
                    );
                }
            }
        }

        loop {
            match spawn_port_forward_manager(machine.clone(), store.clone()).await {
                Ok(Ok(())) => {
                    tracing::warn!(
                        extension = EXTENSION_PORT_FORWARD,
                        "port-forward manager exited unexpectedly, restarting"
                    );
                }
                Ok(Err(err)) => {
                    tracing::warn!(
                        extension = EXTENSION_PORT_FORWARD,
                        error = %err,
                        "port-forward manager failed, restarting"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        extension = EXTENSION_PORT_FORWARD,
                        error = %err,
                        "port-forward manager join failed, restarting"
                    );
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    })
}

fn docker_host_socket_path(inst: &Instance) -> std::path::PathBuf {
    inst.dir()
        .join("sock")
        .join(format!("{}.sock", EXTENSION_DOCKER))
}

fn remove_stale_socket(path: &std::path::Path) -> eyre::Result<()> {
    if let Err(err) = std::fs::remove_file(path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err).context(format!("remove stale socket {}", path.display()));
        }
    }

    Ok(())
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
