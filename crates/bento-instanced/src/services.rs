use std::pin::Pin;
use std::sync::Arc;

use bento_protocol::instance::v1::instance_control_service_server::{
    InstanceControlService, InstanceControlServiceServer,
};
use bento_protocol::instance::v1::{
    GetStatusRequest, GetStatusResponse, HealthRequest, HealthResponse, StatusUpdate,
    WatchStatusRequest,
};
use futures::stream::{self, Stream, StreamExt};
use tokio::net::UnixStream;
use tokio::sync::broadcast;
use tonic::{Request, Response, Status};

use crate::state::{
    select_current_events, select_current_health, select_current_status, InstanceStore,
};

type WatchStatusStream = Pin<Box<dyn Stream<Item = Result<StatusUpdate, Status>> + Send>>;

#[derive(Clone)]
struct InstanceControlSvc {
    store: Arc<InstanceStore>,
}

#[tonic::async_trait]
impl InstanceControlService for InstanceControlSvc {
    type WatchStatusStream = WatchStatusStream;

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let snapshot = self.store.snapshot().unwrap_or_default();
        let response = select_current_health(&snapshot);

        tracing::info!(
            service = "instance_control.health",
            ok = response.ok,
            message = %response.message,
            "instance control health request"
        );

        Ok(Response::new(response))
    }

    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let snapshot = self.store.snapshot().unwrap_or_default();
        Ok(Response::new(select_current_status(&snapshot)))
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
        .add_service(InstanceControlServiceServer::new(InstanceControlSvc {
            store,
        }))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
