use std::sync::Arc;
use std::time::Duration;

use bento_machine::Machine;
use bento_protocol::instance::v1::{ExtensionStatus, HostSocket, LifecycleState};
use bento_runtime::extensions::{BuiltinExtension, EXTENSION_DOCKER};
use bento_runtime::instance::{Instance, InstanceFile};
use bento_runtime::instance_store::InstanceStore;
use bento_runtime::services::SERVICE_DOCKER;
use eyre::Context;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;

use crate::discovery::ServiceRegistry;
use crate::host_export::listen_host_service;
use crate::machine::machine_spec_for_instance;
use crate::pid_guard::PidGuard;
use crate::serial::SerialConsole;
use crate::server::InstanceServer;
use crate::state::{new_instance_store, Action};

const GUEST_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const GUEST_DISCOVERY_RETRY: Duration = Duration::from_secs(1);

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

        remove_stale_socket(&inst.file(InstanceFile::InstancedSocket))?;

        let machine_spec = machine_spec_for_instance(&inst)?;
        let machine = Machine::create_or_get(machine_spec.clone()).await?;
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

        store.dispatch(Action::vm_starting());

        machine.start().await?;

        if let Err(err) = serial_console
            .stream_to_file(&inst.file(InstanceFile::SerialLog))
            .await
        {
            let _ = Machine::release(&machine_spec.id).await;
            return Err(err);
        }

        store.dispatch(Action::vm_running());

        tracing::info!(instance = %self.name, "instanced running");

        let mut sigint = signal(SignalKind::interrupt()).context("register SIGINT handler")?;
        let mut sigterm = signal(SignalKind::terminate()).context("register SIGTERM handler")?;

        if expects_guest_agent {
            store.dispatch(Action::guest_starting());
            spawn_guest_extension_monitor(inst.clone(), machine.clone(), store.clone());
        } else {
            store.dispatch(Action::set_extensions(Vec::new()));
            store.dispatch(Action::guest_running());
        }

        tokio::select! {
            _ = sigint.recv() => {
                tracing::info!(instance = %self.name, "received SIGINT, shutting down instanced");
                store.dispatch(Action::VmTransition {
                    state: bento_protocol::instance::v1::LifecycleState::Stopping,
                    message: String::from("received SIGINT"),
                });
            }
            _ = sigterm.recv() => {
                tracing::info!(instance = %self.name, "received SIGTERM, shutting down instanced");
                store.dispatch(Action::VmTransition {
                    state: bento_protocol::instance::v1::LifecycleState::Stopping,
                    message: String::from("received SIGTERM"),
                });
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
            }
        }

        tracing::info!(instance = %self.name, "sending stop signal to vm");

        Machine::release(&machine_spec.id).await?;

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
    machine: bento_machine::MachineHandle,
    store: Arc<crate::state::InstanceStore>,
) {
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
    });
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
