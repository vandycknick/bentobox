use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bento_core::services::RESERVED_SHELL_PORT;
use bento_protocol::negotiate::Upgrade;
use bento_protocol::v1::vm_monitor_service_server::{VmMonitorService, VmMonitorServiceServer};
use bento_protocol::v1::{
    InspectRequest, InspectResponse, PingRequest, PingResponse, StatusUpdate, WatchStatusRequest,
};
use bento_vmm::{spawn_serial_tunnel, SerialAccess};
use eyre::Context;
use futures::stream::{self, Stream, StreamExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::agent::AgentClient;
use crate::context::{DaemonContext, RuntimeContext};
use crate::endpoints::start_endpoint_supervisor;
use crate::net::server::NegotiateServer;
use crate::net::tunnel::spawn_tunnel;
use crate::startup::StartupReporter;
use crate::state::{
    guest_shell_ready as state_guest_shell_ready, select_current_events, select_current_inspect,
    select_current_ping, Action, InstanceStore,
};

const GUEST_HEALTH_TIMEOUT: Duration = Duration::from_secs(60 * 5);

type WatchStatusStream = Pin<Box<dyn Stream<Item = Result<StatusUpdate, Status>> + Send>>;

pub struct ServiceHandles {
    pub(crate) control_socket: JoinHandle<eyre::Result<()>>,
    pub(crate) guest_monitor: Option<JoinHandle<()>>,
    pub(crate) endpoint_supervisor: Option<JoinHandle<()>>,
    pub(crate) serial_log: JoinHandle<()>,
}

#[derive(Clone)]
struct VmMonitorSvc {
    store: Arc<InstanceStore>,
}

#[tonic::async_trait]
impl VmMonitorService for VmMonitorSvc {
    type WatchStatusStream = WatchStatusStream;

    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let snapshot = self.store.snapshot().unwrap_or_default();
        let response = select_current_ping(&snapshot);

        tracing::info!(
            service = "vm_monitor.ping",
            ok = response.ok,
            message = %response.message,
            "vm monitor ping request"
        );

        Ok(Response::new(response))
    }

    async fn inspect(
        &self,
        _request: Request<InspectRequest>,
    ) -> Result<Response<InspectResponse>, Status> {
        let snapshot = self.store.snapshot().unwrap_or_default();
        Ok(Response::new(select_current_inspect(&snapshot)))
    }

    async fn watch_status(
        &self,
        _request: Request<WatchStatusRequest>,
    ) -> Result<Response<Self::WatchStatusStream>, Status> {
        let snapshot = self.store.snapshot().unwrap_or_default();
        let snapshots = select_current_events(&snapshot);
        let rx = self.store.subscribe();

        let snapshot_stream = stream::iter(snapshots.into_iter().map(Ok));
        let update_stream = stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(update) => Some((Ok(update), rx)),
                Err(broadcast::error::RecvError::Lagged(skipped)) => Some((
                    Err(Status::resource_exhausted(format!(
                        "status stream lagged, skipped {skipped} updates"
                    ))),
                    rx,
                )),
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });

        Ok(Response::new(Box::pin(
            snapshot_stream.chain(update_stream),
        )))
    }
}

pub async fn start_services(
    runtime: &RuntimeContext,
    ctx: &DaemonContext,
    startup_reporter: &mut StartupReporter,
) -> eyre::Result<ServiceHandles> {
    let path = runtime.file(bento_core::InstanceFile::VmmonSocket);
    let listener = UnixListener::bind(&path).context(format!("bind socket {}", path.display()))?;
    let server = NegotiateServer::new(listener, ctx.shutdown.clone());
    let handler_ctx = ctx.clone();
    let control_socket = server.listen(move |stream, upgrade| {
        let ctx = handler_ctx.clone();
        async move { handle_connection(stream, upgrade, ctx).await }
    });

    let serial_log_path = runtime.file(bento_core::InstanceFile::SerialLog);
    let serial_console_for_log = ctx.serial_console.clone();
    let serial_log = tokio::spawn(async move {
        if let Err(err) = serial_console_for_log
            .stream_to_file(&serial_log_path)
            .await
        {
            tracing::warn!(error = %err, path = %serial_log_path.display(), "serial log attachment failed");
        }
    });

    let guest_monitor = if ctx.spec.settings.guest_enabled {
        ctx.store.dispatch(Action::guest_starting());
        Some(spawn_agent_monitor(
            AgentClient::new(&ctx.machine, &ctx.spec),
            ctx.store.clone(),
            ctx.shutdown.clone(),
        ))
    } else {
        ctx.store.dispatch(Action::guest_running());
        None
    };

    let endpoint_supervisor = start_endpoint_supervisor(ctx.clone(), runtime.dir().to_path_buf());

    startup_reporter.report_started()?;
    tracing::info!(instance = %ctx.machine.name(), "vmmon running");

    Ok(ServiceHandles {
        control_socket,
        guest_monitor,
        endpoint_supervisor,
        serial_log,
    })
}

pub(crate) async fn serve(stream: UnixStream, store: Arc<InstanceStore>) -> eyre::Result<()> {
    let incoming = stream::once(async move { Ok::<_, std::io::Error>(stream) });
    tonic::transport::Server::builder()
        .add_service(VmMonitorServiceServer::new(VmMonitorSvc { store }))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    upgrade: Upgrade,
    ctx: DaemonContext,
) -> eyre::Result<()> {
    match upgrade {
        Upgrade::Serial => {
            let serial_stream = ctx
                .serial_console
                .open_stream(SerialAccess::Interactive)
                .await?;
            spawn_serial_tunnel(stream, serial_stream);
            Ok(())
        }
        Upgrade::Shell => {
            if !guest_shell_ready(&ctx.store) {
                tracing::warn!("shell requested before guest shell was ready, closing connection");
                return Ok(());
            }

            match ctx.machine.connect_vsock(RESERVED_SHELL_PORT).await {
                Ok(vsock_stream) => {
                    spawn_tunnel(stream, vsock_stream);
                    Ok(())
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to connect shell backend, closing connection");
                    Ok(())
                }
            }
        }
        Upgrade::Api { .. } => serve(stream, ctx.store).await,
    }
}

fn spawn_agent_monitor(
    agent: AgentClient,
    store: Arc<InstanceStore>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let mut health_stream = Box::pin(agent.watch(shutdown));

    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + GUEST_HEALTH_TIMEOUT;

        while let Some(result) = health_stream.next().await {
            if let Err(err) = handle_agent_update(&store, result, deadline) {
                tracing::warn!(error = %err, timeout = ?GUEST_HEALTH_TIMEOUT, "guest services did not become ready before timeout");
                store.dispatch(Action::guest_error(format!(
                    "guest health check failed: {err}"
                )));
                return;
            }
        }

        tracing::info!("agent monitor shutting down");
    })
}

fn handle_agent_update(
    store: &InstanceStore,
    result: eyre::Result<bento_protocol::v1::HealthResponse>,
    deadline: tokio::time::Instant,
) -> Result<(), eyre::Report> {
    match result {
        Ok(health) => {
            let waiting_summary = health.summary.clone();
            store.dispatch(Action::set_services(health.services));

            if health.ready {
                store.dispatch(Action::guest_running());
            } else {
                store.dispatch(Action::guest_starting());
                tracing::warn!(reason = %waiting_summary, "startup-required guest services not ready yet");
            }

            Ok(())
        }
        Err(err) if tokio::time::Instant::now() >= deadline => Err(err),
        Err(err) => {
            tracing::info!(reason = %classify_health_retry(&err), "agent not ready yet");
            tracing::debug!(error = %err, "agent retry detail");
            Ok(())
        }
    }
}

fn classify_health_retry(err: &eyre::Report) -> &'static str {
    let message = err.to_string().to_ascii_lowercase();

    if message.contains("unimplemented") {
        return "agent protocol is older than vmmon";
    }

    if message.contains("connection reset by peer")
        || message.contains("connection refused")
        || message.contains("not connected")
        || message.contains("service unavailable")
    {
        return "agent is not reachable yet";
    }

    if message.contains("timed out") {
        return "agent rpc timed out";
    }

    "waiting for agent rpc"
}

fn guest_shell_ready(store: &InstanceStore) -> bool {
    let Some(snapshot) = store.snapshot() else {
        return false;
    };

    state_guest_shell_ready(&snapshot)
}
