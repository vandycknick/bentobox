use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bento_protocol::instance::v1::instance_control_service_server::{
    InstanceControlService, InstanceControlServiceServer,
};
use bento_protocol::instance::v1::{
    HealthRequest, HealthResponse, LifecycleState, StatusSource, StatusUpdate, WatchStatusRequest,
};
use futures::stream::{self, Stream, StreamExt};
use tokio::net::UnixStream;
use tokio::sync::broadcast;
use tonic::{Request, Response, Status};

#[derive(Debug)]
struct StatusSnapshot {
    vm_state: LifecycleState,
    guest_state: LifecycleState,
}

#[derive(Debug, Clone)]
pub(crate) struct InstanceControlState {
    status_tx: broadcast::Sender<StatusUpdate>,
    snapshot: Arc<Mutex<StatusSnapshot>>,
}

impl InstanceControlState {
    pub(crate) fn new() -> Self {
        let (status_tx, _) = broadcast::channel(256);
        Self {
            status_tx,
            snapshot: Arc::new(Mutex::new(StatusSnapshot {
                vm_state: LifecycleState::Unspecified,
                guest_state: LifecycleState::Unspecified,
            })),
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<StatusUpdate> {
        self.status_tx.subscribe()
    }

    pub(crate) fn publish_vm_state(&self, state: LifecycleState, message: impl Into<String>) {
        self.publish(StatusSource::Vm, state, message);
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.vm_state = state;
        }
    }

    pub(crate) fn publish_guest_state(&self, state: LifecycleState, message: impl Into<String>) {
        self.publish(StatusSource::Guest, state, message);
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.guest_state = state;
        }
    }

    pub(crate) fn health_response(&self) -> HealthResponse {
        let snapshot = self
            .snapshot
            .lock()
            .map(|guard| (guard.vm_state, guard.guest_state))
            .unwrap_or((LifecycleState::Unspecified, LifecycleState::Unspecified));

        let ok = snapshot.1 == LifecycleState::Running;
        HealthResponse {
            ok,
            message: if ok {
                String::new()
            } else {
                format!(
                    "guest not ready (vm_state={:?}, guest_state={:?})",
                    snapshot.0, snapshot.1
                )
            },
        }
    }

    pub(crate) fn snapshot_events(&self) -> Vec<StatusUpdate> {
        let snapshot = self
            .snapshot
            .lock()
            .map(|guard| (guard.vm_state, guard.guest_state))
            .unwrap_or((LifecycleState::Unspecified, LifecycleState::Unspecified));

        let mut events = Vec::new();
        if snapshot.0 != LifecycleState::Unspecified {
            events.push(make_status_update(
                StatusSource::Vm,
                snapshot.0,
                String::new(),
            ));
        }
        if snapshot.1 != LifecycleState::Unspecified {
            events.push(make_status_update(
                StatusSource::Guest,
                snapshot.1,
                String::new(),
            ));
        }
        events
    }

    fn publish(&self, source: StatusSource, state: LifecycleState, message: impl Into<String>) {
        let _ = self
            .status_tx
            .send(make_status_update(source, state, message.into()));
    }
}

type WatchStatusStream = Pin<Box<dyn Stream<Item = Result<StatusUpdate, Status>> + Send>>;

#[derive(Clone)]
struct InstanceControlSvc {
    state: Arc<InstanceControlState>,
}

#[tonic::async_trait]
impl InstanceControlService for InstanceControlSvc {
    type WatchStatusStream = WatchStatusStream;

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let response = self.state.health_response();

        tracing::info!(
            service = "instance_control.health",
            ok = response.ok,
            message = %response.message,
            "instance control health request"
        );

        Ok(Response::new(response))
    }

    async fn watch_status(
        &self,
        _request: Request<WatchStatusRequest>,
    ) -> Result<Response<Self::WatchStatusStream>, Status> {
        let snapshots = self.state.snapshot_events();
        let rx = self.state.subscribe();

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

pub(crate) async fn serve(
    stream: UnixStream,
    state: Arc<InstanceControlState>,
) -> eyre::Result<()> {
    let incoming = stream::once(async move { Ok::<_, std::io::Error>(stream) });
    tonic::transport::Server::builder()
        .add_service(InstanceControlServiceServer::new(InstanceControlSvc {
            state,
        }))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}

fn make_status_update(
    source: StatusSource,
    state: LifecycleState,
    message: String,
) -> StatusUpdate {
    StatusUpdate {
        source: source as i32,
        state: state as i32,
        message,
        timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    }
}
