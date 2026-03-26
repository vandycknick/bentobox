use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bento_protocol::guest::v1::guest_discovery_service_client::GuestDiscoveryServiceClient;
use bento_protocol::guest::v1::{PortForwardEvent, PortForwardEventType, WatchPortForwardsRequest};
use bento_protocol::instance::v1::PortForwardStatus;
use bento_protocol::DEFAULT_DISCOVERY_PORT;
use bento_vmm::VirtualMachine;
use eyre::Context;
use hyper_util::rt::TokioIo;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tonic::transport::Endpoint;
use tower::service_fn;

use crate::state::{Action, InstanceStore};

const RECONNECT_DELAY: Duration = Duration::from_secs(1);

struct RunningHostForward {
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RunningHostForward {
    fn new(machine: VirtualMachine, guest_port: u32, vsock_port: u32) -> eyre::Result<Self> {
        let host_port = u16::try_from(guest_port)
            .map_err(|_| eyre::eyre!("guest port {guest_port} is out of host tcp range"))?;
        let listener = std::net::TcpListener::bind(("127.0.0.1", host_port)).map_err(|err| {
            eyre::eyre!("bind host port forward 127.0.0.1:{guest_port} failed: {err}")
        })?;
        listener
            .set_nonblocking(true)
            .context("set host port listener nonblocking")?;
        let listener = TcpListener::from_std(listener).context("adopt host tcp listener")?;

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                let machine = machine.clone();
                                tokio::spawn(async move {
                                    if let Err(err) = handle_host_connection(machine, stream, vsock_port).await {
                                        tracing::warn!(
                                            error = %err,
                                            guest_port,
                                            vsock_port,
                                            "host port-forward connection failed"
                                        );
                                    }
                                });
                            }
                            Err(err) => {
                                tracing::warn!(error = %err, guest_port, "host port-forward accept failed");
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
        })
    }

    fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    async fn shutdown_and_wait(&mut self, guest_port: u32) {
        self.shutdown();
        if let Some(task) = self.task.take() {
            if let Err(err) = task.await {
                tracing::warn!(error = %err, guest_port, "host port-forward shutdown join failed");
            }
        }
    }
}

pub(crate) fn spawn_port_forward_manager(
    machine: VirtualMachine,
    store: Arc<InstanceStore>,
) -> tokio::task::JoinHandle<eyre::Result<()>> {
    tokio::spawn(async move {
        let mut active_forwards = BTreeMap::<u32, RunningHostForward>::new();
        let mut statuses = BTreeMap::<u32, PortForwardStatus>::new();
        store.dispatch(Action::set_port_forwards(Vec::new()));

        loop {
            let mut client = match connect_guest_client(&machine).await {
                Ok(client) => client,
                Err(err) => {
                    tracing::warn!(error = %err, "connect guest discovery for port-forward failed");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                    continue;
                }
            };

            let mut updates = match client
                .watch_port_forwards(WatchPortForwardsRequest {})
                .await
            {
                Ok(response) => response.into_inner(),
                Err(err) => {
                    tracing::warn!(error = %err, "watch_port_forwards rpc failed");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                    continue;
                }
            };

            loop {
                match updates.message().await {
                    Ok(Some(event)) => {
                        apply_event(
                            &machine,
                            &mut active_forwards,
                            &mut statuses,
                            store.as_ref(),
                            event,
                        )
                        .await;
                    }
                    Ok(None) => {
                        tracing::warn!("watch_port_forwards stream closed, reconnecting");
                        break;
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "watch_port_forwards stream error, reconnecting");
                        break;
                    }
                }
            }

            for (guest_port, forward) in active_forwards.iter_mut() {
                tracing::info!(
                    guest_port = *guest_port,
                    "tearing down host port forward due to stream reconnect"
                );
                forward.shutdown_and_wait(*guest_port).await;
                tracing::info!(
                    guest_port = *guest_port,
                    "host port forward torn down due to stream reconnect"
                );
            }
            active_forwards.clear();
            statuses.clear();
            store.dispatch(Action::set_port_forwards(Vec::new()));
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    })
}

async fn apply_event(
    machine: &VirtualMachine,
    active_forwards: &mut BTreeMap<u32, RunningHostForward>,
    statuses: &mut BTreeMap<u32, PortForwardStatus>,
    store: &InstanceStore,
    event: PortForwardEvent,
) {
    let Some(forward) = event.forward else {
        tracing::warn!("ignoring port-forward event without payload");
        return;
    };

    let guest_port = forward.guest_port;
    let vsock_port = forward.vsock_port;
    let event_type = PortForwardEventType::try_from(event.event_type)
        .unwrap_or(PortForwardEventType::Unspecified);

    match event_type {
        PortForwardEventType::Added => {
            tracing::info!(
                guest_port,
                vsock_port,
                "received port-forward add event from guestd"
            );
            if let Some(mut existing) = active_forwards.remove(&guest_port) {
                existing.shutdown_and_wait(guest_port).await;
                tracing::info!(
                    guest_port,
                    "teared down stale host forward before re-adding"
                );
            }

            match RunningHostForward::new(machine.clone(), guest_port, vsock_port) {
                Ok(host_forward) => {
                    active_forwards.insert(guest_port, host_forward);
                    statuses.insert(
                        guest_port,
                        PortForwardStatus {
                            guest_port,
                            host_port: guest_port,
                            active: true,
                            message: format!(
                                "forwarding localhost:{guest_port} to guest port {guest_port}"
                            ),
                        },
                    );
                    publish_status_snapshot(statuses, store);
                    tracing::info!(
                        guest_port,
                        host_port = guest_port,
                        vsock_port,
                        "added host port forward"
                    );
                }
                Err(err) => {
                    statuses.insert(
                        guest_port,
                        PortForwardStatus {
                            guest_port,
                            host_port: guest_port,
                            active: false,
                            message: err.to_string(),
                        },
                    );
                    publish_status_snapshot(statuses, store);
                    tracing::error!(
                        error = %err,
                        guest_port,
                        host_port = guest_port,
                        "host port is unavailable, skipping port forward"
                    );
                }
            }
        }
        PortForwardEventType::Removed => {
            tracing::info!(
                guest_port,
                vsock_port,
                "received port-forward remove event from guestd"
            );
            if let Some(mut existing) = active_forwards.remove(&guest_port) {
                existing.shutdown_and_wait(guest_port).await;
                tracing::info!(guest_port, "removed host port forward");
            }
            statuses.remove(&guest_port);
            publish_status_snapshot(statuses, store);
        }
        PortForwardEventType::Unspecified => {
            tracing::warn!(guest_port, "ignoring unspecified port-forward event");
        }
    }
}

fn publish_status_snapshot(statuses: &BTreeMap<u32, PortForwardStatus>, store: &InstanceStore) {
    store.dispatch(Action::set_port_forwards(
        statuses.values().cloned().collect(),
    ));
}

async fn connect_guest_client(
    machine: &VirtualMachine,
) -> eyre::Result<GuestDiscoveryServiceClient<tonic::transport::Channel>> {
    let stream = machine.connect_vsock(DEFAULT_DISCOVERY_PORT).await?;
    let stream_slot = Arc::new(Mutex::new(Some(stream)));
    let connector = service_fn(move |_| {
        let stream_slot = Arc::clone(&stream_slot);
        async move {
            let mut guard = stream_slot.lock().await;
            guard
                .take()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "guest discovery connector stream already consumed",
                    )
                })
                .map(TokioIo::new)
        }
    });

    let channel = Endpoint::from_static("http://guest-discovery.local")
        .connect_with_connector(connector)
        .await
        .context("connect guest discovery rpc client")?;

    Ok(GuestDiscoveryServiceClient::new(channel))
}

async fn handle_host_connection(
    machine: VirtualMachine,
    mut host_stream: TcpStream,
    vsock_port: u32,
) -> io::Result<()> {
    let mut vsock_stream = machine
        .connect_vsock(vsock_port)
        .await
        .map_err(|err| io::Error::other(err.to_string()))?;
    let _ = copy_bidirectional(&mut host_stream, &mut vsock_stream).await?;
    Ok(())
}
