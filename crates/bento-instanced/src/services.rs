use std::pin::Pin;
use std::sync::Arc;

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
use tonic::{Request, Response, Status};

use crate::context::DaemonContext;
use crate::guest;
use crate::net::server::NegotiateServer;
use crate::startup_reporter::StartupReporter;
use crate::state::{
    guest_shell_ready as state_guest_shell_ready, select_current_events, select_current_inspect,
    select_current_ping, Action, InstanceStore,
};
use crate::tunnel::spawn_tunnel;

type WatchStatusStream = Pin<Box<dyn Stream<Item = Result<StatusUpdate, Status>> + Send>>;

pub struct ServiceHandles {
    pub(crate) control_socket: JoinHandle<eyre::Result<()>>,
    pub(crate) guest_monitor: Option<JoinHandle<()>>,
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
    ctx: &DaemonContext,
    startup_reporter: &mut StartupReporter,
) -> eyre::Result<ServiceHandles> {
    let path = ctx.vm.file(bento_core::InstanceFile::InstancedSocket);
    let listener = UnixListener::bind(&path).context(format!("bind socket {}", path.display()))?;
    let server = NegotiateServer::new(listener, ctx.shutdown.clone());
    let handler_ctx = ctx.clone();
    let control_socket = server.listen(move |stream, upgrade| {
        let ctx = handler_ctx.clone();
        async move { handle_connection(stream, upgrade, ctx).await }
    });

    let serial_log_path = ctx.vm.file(bento_core::InstanceFile::SerialLog);
    let serial_console_for_log = ctx.serial_console.clone();
    let serial_log = tokio::spawn(async move {
        if let Err(err) = serial_console_for_log
            .stream_to_file(&serial_log_path)
            .await
        {
            tracing::warn!(error = %err, path = %serial_log_path.display(), "serial log attachment failed");
        }
    });

    let guest_monitor = if ctx.guest_enabled {
        ctx.store.dispatch(Action::guest_starting());
        Some(guest::spawn_service_monitor(
            ctx.machine.clone(),
            ctx.store.clone(),
            ctx.shutdown.clone(),
        ))
    } else {
        ctx.store.dispatch(Action::guest_running());
        None
    };

    startup_reporter.report_started()?;
    tracing::info!(instance = %ctx.vm.name, "vmmon running");

    Ok(ServiceHandles {
        control_socket,
        guest_monitor,
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

fn guest_shell_ready(store: &InstanceStore) -> bool {
    let Some(snapshot) = store.snapshot() else {
        return false;
    };

    state_guest_shell_ready(&snapshot)
}
