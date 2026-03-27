use std::sync::Arc;
use std::time::Duration;

use bento_protocol::v1::{CapabilityStatus, EndpointKind, EndpointStatus, LifecycleState};
use bento_runtime::capabilities::CapabilitiesConfig;
use bento_runtime::instance::{Instance, InstanceFile};
use bento_runtime::instance_store::InstanceStore;
use bento_runtime::profiles::{resolve_profiles, validate_capabilities};
use bento_vmm::{VmExit, Vmm};
use eyre::Context;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;

use crate::bootstrap::rebuild_bootstrap;
use crate::discovery::ServiceRegistry;
use crate::host_export::listen_host_service;
use crate::machine::{instance_machine_config, machine_backend, machine_identifier_path};
use crate::pid_guard::PidGuard;
use crate::port_forward::spawn_port_forward_manager;
use crate::server::InstanceServer;
use crate::state::{new_instance_store, Action};

const GUEST_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const GUEST_DISCOVERY_RETRY: Duration = Duration::from_secs(1);
enum LoopExit {
    Signal(&'static str),
    InternalExit(&'static str),
    VmStopped(VmStopInfo),
}

struct VmStopInfo {
    message: String,
}

pub struct InstanceDaemon {
    name: String,
    store: InstanceStore,
    profiles: Vec<String>,
}

impl InstanceDaemon {
    pub fn new(name: &str, profiles: Vec<String>) -> Self {
        Self {
            name: String::from(name),
            store: InstanceStore::new(),
            profiles,
        }
    }

    pub async fn run(&self) -> eyre::Result<()> {
        let inst = self.store.inspect(&self.name)?;
        let resolved_capabilities = resolve_startup_capabilities(&inst, &self.profiles)?;
        let _trace_guard = init_tracing(&inst.file(InstanceFile::InstancedTraceLog))?;
        tracing::info!(instance = %self.name, "instanced starting");

        rebuild_bootstrap(&inst, &resolved_capabilities)?;

        remove_stale_socket(&inst.file(InstanceFile::InstancedSocket))?;

        let machine_config = instance_machine_config(&inst)?;
        let vmm = Vmm::new(machine_backend(inst.engine())?)?;
        let machine = vmm.create(machine_config.config).await?;
        if let Some(machine_identifier) = machine_config.machine_identifier.as_ref() {
            if machine_identifier.was_generated() {
                std::fs::write(machine_identifier_path(&inst), machine_identifier.bytes())?;
            }
        }
        let serial_console = machine.serial();
        let store = Arc::new(new_instance_store());
        let expects_guest_agent = inst.requires_bootstrap_for(&resolved_capabilities);

        let server = InstanceServer::new(machine.clone(), serial_console.clone(), store.clone());
        let server_task = server.listen(&inst.file(InstanceFile::InstancedSocket))?;

        let host_socket_exports = configured_host_socket_exports(&inst, &resolved_capabilities);
        for export in &host_socket_exports {
            let path = &export.host_path;
            remove_stale_socket(path)?;
        }
        store.dispatch(Action::set_static_endpoints(
            configured_static_endpoint_statuses(&resolved_capabilities, &host_socket_exports),
        ));

        let endpoint_export_tasks = host_socket_exports
            .iter()
            .map(|export| {
                listen_host_service(machine.clone(), &export.host_path, export.name.clone())
            })
            .collect::<eyre::Result<Vec<_>>>()?;
        let mut endpoint_export_join_set = tokio::task::JoinSet::new();
        for task in endpoint_export_tasks {
            endpoint_export_join_set.spawn(task);
        }

        let _pid_guard = PidGuard::create(&inst.file(InstanceFile::InstancedPid)).await?;

        let _port_forward_task = if resolved_capabilities.forward.enabled
            && resolved_capabilities.forward.tcp.auto_discover
        {
            Some(spawn_port_forward_manager_when_guest_ready(
                machine.clone(),
                store.clone(),
            ))
        } else {
            store.dispatch(Action::set_dynamic_endpoints(Vec::new()));
            None
        };

        store.dispatch(Action::vm_starting());

        machine.start().await?;

        if let Err(err) = serial_console
            .stream_to_file(&inst.file(InstanceFile::SerialLog))
            .await
        {
            let _ = machine.stop().await;
            return Err(err.into());
        }

        store.dispatch(Action::vm_running());

        tracing::info!(instance = %self.name, "instanced running");

        let mut sigint = signal(SignalKind::interrupt()).context("register SIGINT handler")?;
        let mut sigterm = signal(SignalKind::terminate()).context("register SIGTERM handler")?;

        let mut guest_capability_monitor_task = if expects_guest_agent {
            store.dispatch(Action::guest_starting());
            Some(spawn_guest_capability_monitor(
                inst.clone(),
                resolved_capabilities.clone(),
                machine.clone(),
                store.clone(),
            ))
        } else {
            store.dispatch(Action::set_capabilities(Vec::new()));
            store.dispatch(Action::guest_running());
            None
        };

        let loop_exit = tokio::select! {
            _ = sigint.recv() => {
                tracing::info!(instance = %self.name, "received SIGINT, shutting down instanced");
                store.dispatch(Action::VmTransition {
                    state: bento_protocol::v1::LifecycleState::Stopping,
                    message: String::from("received SIGINT"),
                });
                LoopExit::Signal("SIGINT")
            }
            _ = sigterm.recv() => {
                tracing::info!(instance = %self.name, "received SIGTERM, shutting down instanced");
                store.dispatch(Action::VmTransition {
                    state: bento_protocol::v1::LifecycleState::Stopping,
                    message: String::from("received SIGTERM"),
                });
                LoopExit::Signal("SIGTERM")
            }
            result = wait_for_machine_stop(&machine) => {
                match result {
                    Ok(event) => {
                        tracing::info!(instance = %self.name, message = %event.message, "machine exited");
                        LoopExit::VmStopped(event)
                    }
                    Err(err) => {
                        tracing::warn!(instance = %self.name, error = %err, "machine wait failed");
                        LoopExit::VmStopped(VmStopInfo {
                            message: format!("machine wait failed: {err}"),
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
                if endpoint_export_join_set.is_empty() {
                    std::future::pending().await
                } else {
                    endpoint_export_join_set.join_next().await.expect("join set not empty")
                }
            } => {
                match result {
                    Ok(Ok(Ok(()))) => tracing::warn!(instance = %self.name, "host endpoint export exited"),
                    Ok(Ok(Err(err))) => return Err(err),
                    Ok(Err(err)) => return Err(eyre::eyre!("host endpoint export join failed: {err}")),
                    Err(err) => return Err(eyre::eyre!("host endpoint export task failed: {err}")),
                }
                LoopExit::InternalExit("host endpoint export exited")
            }
        };

        if let Some(task) = guest_capability_monitor_task.take() {
            task.abort();
        }

        let vm_already_stopped = matches!(loop_exit, LoopExit::VmStopped(_));

        if let LoopExit::Signal(reason) = &loop_exit {
            tracing::info!(instance = %self.name, reason, "beginning shutdown flow");
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

        for export in host_socket_exports {
            let _ = std::fs::remove_file(&export.host_path);
        }

        tracing::info!(instance = %self.name, "instance stopped");

        Ok(())
    }
}

fn spawn_guest_capability_monitor(
    inst: Instance,
    resolved_capabilities: CapabilitiesConfig,
    machine: bento_vmm::VirtualMachine,
    store: Arc<crate::state::InstanceStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + GUEST_DISCOVERY_TIMEOUT;
        let mut guest_probe_tick = tokio::time::interval(GUEST_DISCOVERY_RETRY);

        loop {
            guest_probe_tick.tick().await;

            match ServiceRegistry::discover(&machine).await {
                Ok(registry) => {
                    let capabilities =
                        project_capability_statuses(&resolved_capabilities, &registry);
                    let endpoints =
                        project_static_endpoint_statuses(&inst, &resolved_capabilities, &registry);
                    let ready = startup_required_capabilities_ready(&capabilities);
                    let waiting_summary = startup_required_wait_summary(&capabilities);
                    store.dispatch(Action::set_capabilities(capabilities));
                    store.dispatch(Action::set_static_endpoints(endpoints));

                    if ready {
                        store.dispatch(Action::guest_running());
                    } else {
                        store.dispatch(Action::guest_starting());
                        tracing::info!(reason = %waiting_summary, "startup-required capabilities not ready yet");
                    }
                }
                Err(err) if tokio::time::Instant::now() >= deadline => {
                    tracing::warn!(error = %err, timeout = ?GUEST_DISCOVERY_TIMEOUT, "guest capabilities did not become ready before timeout");
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

async fn wait_for_machine_stop(
    machine: &bento_vmm::VirtualMachine,
) -> Result<VmStopInfo, eyre::Report> {
    let exit = machine.wait().await?;
    let message = match exit {
        VmExit::Stopped => String::from("machine stopped"),
        VmExit::StoppedWithError(error) => format!("machine stopped with error: {error}"),
    };
    Ok(VmStopInfo { message })
}

fn project_capability_statuses(
    capabilities: &CapabilitiesConfig,
    registry: &ServiceRegistry,
) -> Vec<CapabilityStatus> {
    enabled_capability_ids(capabilities)
        .into_iter()
        .map(|capability_name| {
            let reported = registry
                .capabilities()
                .find(|status| status.name == capability_name);
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
                    format!("{capability_name} was not reported by guestd"),
                    vec![format!("guestd did not report {capability_name}")],
                ),
            };

            CapabilityStatus {
                name: capability_name.to_string(),
                enabled: true,
                startup_required: capabilities
                    .startup_required_capabilities()
                    .contains(&capability_name),
                configured,
                running,
                summary,
                problems,
            }
        })
        .collect()
}

fn project_static_endpoint_statuses(
    inst: &Instance,
    capabilities: &CapabilitiesConfig,
    registry: &ServiceRegistry,
) -> Vec<EndpointStatus> {
    registry
        .endpoints()
        .map(|endpoint| EndpointStatus {
            name: endpoint.name.clone(),
            kind: endpoint.kind,
            guest_address: endpoint.guest_address.clone(),
            host_address: host_address_for_endpoint(inst, capabilities, &endpoint.name)
                .unwrap_or_default(),
            active: true,
            summary: format!("guest endpoint {} is available", endpoint.name),
            problems: Vec::new(),
        })
        .collect()
}

fn startup_required_capabilities_ready(capabilities: &[CapabilityStatus]) -> bool {
    capabilities
        .iter()
        .filter(|capability| capability.startup_required)
        .all(|capability| capability.configured && capability.running)
}

fn startup_required_wait_summary(capabilities: &[CapabilityStatus]) -> String {
    let reasons = capabilities
        .iter()
        .filter(|capability| capability.startup_required)
        .filter(|capability| !(capability.configured && capability.running))
        .map(|capability| {
            let detail = capability
                .problems
                .first()
                .cloned()
                .filter(|problem| !problem.is_empty())
                .unwrap_or_else(|| capability.summary.clone());
            format!("{}: {}", capability.name, detail)
        })
        .collect::<Vec<_>>();

    if reasons.is_empty() {
        String::from("waiting for startup-required capabilities")
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
    machine: bento_vmm::VirtualMachine,
    store: Arc<crate::state::InstanceStore>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut guest_probe_tick = tokio::time::interval(GUEST_DISCOVERY_RETRY);

        loop {
            guest_probe_tick.tick().await;
            match ServiceRegistry::discover(&machine).await {
                Ok(_) => {
                    tracing::info!(
                        capability = "forward",
                        "guest discovery is ready, starting tcp forward manager"
                    );
                    break;
                }
                Err(err) => {
                    tracing::info!(
                        capability = "forward",
                        reason = %classify_guest_discovery_retry(&err),
                        "tcp forward waiting for guest discovery"
                    );
                    tracing::debug!(
                        capability = "forward",
                        error = %err,
                        "tcp forward guest discovery retry detail"
                    );
                }
            }
        }

        loop {
            match spawn_port_forward_manager(machine.clone(), store.clone()).await {
                Ok(Ok(())) => {
                    tracing::warn!(
                        capability = "forward",
                        "tcp forward manager exited unexpectedly, restarting"
                    );
                }
                Ok(Err(err)) => {
                    tracing::warn!(
                        capability = "forward",
                        error = %err,
                        "tcp forward manager failed, restarting"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        capability = "forward",
                        error = %err,
                        "tcp forward manager join failed, restarting"
                    );
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    })
}

fn enabled_capability_ids(capabilities: &CapabilitiesConfig) -> Vec<&'static str> {
    let mut ids = Vec::new();
    if capabilities.ssh.enabled {
        ids.push("ssh");
    }
    if capabilities.dns.enabled {
        ids.push("dns");
    }
    if capabilities.forward.enabled {
        ids.push("forward");
    }
    ids
}

struct HostSocketExport {
    name: String,
    guest_path: String,
    host_path: std::path::PathBuf,
}

fn configured_host_socket_exports(
    inst: &Instance,
    capabilities: &CapabilitiesConfig,
) -> Vec<HostSocketExport> {
    capabilities
        .forward
        .uds
        .iter()
        .map(|forward| HostSocketExport {
            name: forward.name.clone(),
            guest_path: forward.guest_path.clone(),
            host_path: inst.dir().join("sock").join(&forward.host_path),
        })
        .collect()
}

fn configured_static_endpoint_statuses(
    capabilities: &CapabilitiesConfig,
    host_socket_exports: &[HostSocketExport],
) -> Vec<EndpointStatus> {
    let mut endpoints = Vec::new();
    if capabilities.ssh.enabled {
        endpoints.push(EndpointStatus {
            name: String::from("ssh"),
            kind: EndpointKind::Ssh as i32,
            guest_address: String::from("127.0.0.1:22"),
            host_address: String::new(),
            active: false,
            summary: String::from("waiting for guest ssh endpoint"),
            problems: Vec::new(),
        });
    }

    endpoints.extend(host_socket_exports.iter().map(|export| EndpointStatus {
        name: export.name.clone(),
        kind: EndpointKind::UnixSocket as i32,
        guest_address: export.guest_path.clone(),
        host_address: export.host_path.display().to_string(),
        active: false,
        summary: format!("host socket {} is configured", export.host_path.display()),
        problems: Vec::new(),
    }));

    endpoints
}

fn host_address_for_endpoint(
    inst: &Instance,
    capabilities: &CapabilitiesConfig,
    endpoint_name: &str,
) -> Option<String> {
    capabilities
        .forward
        .uds
        .iter()
        .find(|forward| forward.name == endpoint_name)
        .map(|forward| {
            inst.dir()
                .join("sock")
                .join(&forward.host_path)
                .display()
                .to_string()
        })
}

fn resolve_startup_capabilities(
    inst: &Instance,
    start_profiles: &[String],
) -> eyre::Result<CapabilitiesConfig> {
    let mut profiles = inst.profiles().to_vec();
    profiles.extend(start_profiles.iter().cloned());
    let capabilities = resolve_profiles(inst.capabilities(), &profiles)?;
    validate_capabilities(&capabilities)?;
    Ok(capabilities)
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
