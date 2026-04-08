use std::sync::Arc;
use std::time::Duration;

use bento_protocol::v1::ServiceHealth;
use tokio_util::sync::CancellationToken;

use crate::guest_control;
use crate::service_config::HostServiceDefinition;
use crate::state::{Action, InstanceStore};

const GUEST_HEALTH_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const GUEST_HEALTH_RETRY: Duration = Duration::from_secs(1);

pub(super) fn spawn_service_monitor(
    machine: bento_vmm::VirtualMachine,
    services: Vec<HostServiceDefinition>,
    store: Arc<InstanceStore>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + GUEST_HEALTH_TIMEOUT;
        let mut guest_probe_tick = tokio::time::interval(GUEST_HEALTH_RETRY);

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("guest service monitor shutting down");
                    return;
                }
                _ = guest_probe_tick.tick() => {}
            }

            match guest_control::health(&machine).await {
                Ok(health) => {
                    let endpoints = project_endpoint_statuses(&services, &health.services);
                    let waiting_summary = health.summary.clone();
                    store.dispatch(Action::set_services(health.services));
                    store.dispatch(Action::set_static_endpoints(endpoints));

                    if health.ready {
                        store.dispatch(Action::guest_running());
                    } else {
                        store.dispatch(Action::guest_starting());
                        tracing::info!(reason = %waiting_summary, "startup-required guest services not ready yet");
                    }
                }
                Err(err) if tokio::time::Instant::now() >= deadline => {
                    tracing::warn!(error = %err, timeout = ?GUEST_HEALTH_TIMEOUT, "guest services did not become ready before timeout");
                    store.dispatch(Action::guest_error(format!(
                        "guest health check failed: {err}"
                    )));
                    return;
                }
                Err(err) => {
                    tracing::info!(reason = %classify_health_retry(&err), "guest control not ready yet");
                    tracing::debug!(error = %err, "guest control retry detail");
                }
            }
        }
    })
}

pub(super) fn project_endpoint_statuses(
    services: &[HostServiceDefinition],
    health: &[ServiceHealth],
) -> Vec<bento_protocol::v1::EndpointStatus> {
    services
        .iter()
        .map(|service| {
            let current = health.iter().find(|candidate| candidate.name == service.id);
            let active = current.map(|candidate| candidate.healthy).unwrap_or(false);
            let summary = current
                .map(|candidate| candidate.summary.clone())
                .unwrap_or_else(|| {
                    format!(
                        "service {} is configured on vsock port {}",
                        service.id, service.port
                    )
                });
            let problems = current
                .map(|candidate| candidate.problems.clone())
                .unwrap_or_default();

            let kind = match service.kind {
                bento_core::services::GuestServiceKind::Ssh => {
                    bento_protocol::v1::EndpointKind::Ssh
                }
                bento_core::services::GuestServiceKind::UnixSocketForward => {
                    bento_protocol::v1::EndpointKind::UnixSocket
                }
            };

            bento_protocol::v1::EndpointStatus {
                name: service.id.clone(),
                kind: kind as i32,
                guest_address: service.target.clone(),
                host_address: service
                    .host_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                active,
                summary,
                problems,
            }
        })
        .collect()
}

fn classify_health_retry(err: &eyre::Report) -> &'static str {
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
        return "guest control rpc timed out";
    }

    "waiting for guest control rpc"
}
