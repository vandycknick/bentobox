use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::guest_control;
use crate::state::{Action, InstanceStore};

const GUEST_HEALTH_TIMEOUT: Duration = Duration::from_secs(60 * 5);
const GUEST_HEALTH_RETRY: Duration = Duration::from_secs(1);

pub(super) fn spawn_service_monitor(
    machine: bento_vmm::VirtualMachine,
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

            match guest_control::probe(&machine).await {
                Ok(health) => {
                    let waiting_summary = health.summary.clone();
                    store.dispatch(Action::set_services(health.services));

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
