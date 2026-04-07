use std::pin::Pin;
use std::sync::Arc;

use bento_protocol::v1::vm_monitor_service_server::{VmMonitorService, VmMonitorServiceServer};
use bento_protocol::v1::{
    InspectRequest, InspectResponse, PingRequest, PingResponse, StatusUpdate, WatchStatusRequest,
};
use futures::stream::{self, Stream, StreamExt};
use tokio::net::UnixStream;
use tokio::sync::broadcast;
use tonic::{Request, Response, Status};

use crate::state::{
    select_current_events, select_current_inspect, select_current_ping, InstanceStore,
};

type WatchStatusStream = Pin<Box<dyn Stream<Item = Result<StatusUpdate, Status>> + Send>>;

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

pub(crate) async fn serve(stream: UnixStream, store: Arc<InstanceStore>) -> eyre::Result<()> {
    let incoming = stream::once(async move { Ok::<_, std::io::Error>(stream) });
    tonic::transport::Server::builder()
        .add_service(VmMonitorServiceServer::new(VmMonitorSvc { store }))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
