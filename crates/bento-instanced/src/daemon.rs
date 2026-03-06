use std::sync::Arc;
use std::time::Duration;

use bento_machine::Machine;
use bento_runtime::instance::InstanceFile;
use bento_runtime::instance_store::InstanceStore;
use eyre::Context;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;

use crate::discovery::ServiceRegistry;
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
        let expects_guest_agent = inst.expects_guest_agent();

        let server = InstanceServer::new(machine.clone(), serial_console.clone(), store.clone());
        let server_task = server.listen(&inst.file(InstanceFile::InstancedSocket))?;

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
            wait_for_guest_ready(&machine, &store).await;
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
        }

        tracing::info!(instance = %self.name, "sending stop signal to vm");

        Machine::release(&machine_spec.id).await?;

        store.dispatch(Action::VmTransition {
            state: bento_protocol::instance::v1::LifecycleState::Stopped,
            message: String::from("vm stopped"),
        });

        tracing::info!(instance = %self.name, "instance stopped");

        Ok(())
    }
}

async fn wait_for_guest_ready(
    machine: &bento_machine::MachineHandle,
    store: &crate::state::InstanceStore,
) {
    let deadline = tokio::time::Instant::now() + GUEST_DISCOVERY_TIMEOUT;
    let mut guest_probe_tick = tokio::time::interval(GUEST_DISCOVERY_RETRY);

    loop {
        guest_probe_tick.tick().await;

        match ServiceRegistry::discover(machine).await {
            Ok(_) => {
                store.dispatch(Action::guest_running());
                return;
            }
            Err(err) if tokio::time::Instant::now() >= deadline => {
                tracing::warn!(error = %err, timeout = ?GUEST_DISCOVERY_TIMEOUT, "guest services did not become ready before timeout");
                return;
            }
            Err(err) => {
                tracing::info!(error = %err, "guest services not ready yet");
            }
        }
    }
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
