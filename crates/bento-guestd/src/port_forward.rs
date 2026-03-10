use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bento_protocol::guest::v1::{PortForward, PortForwardEvent, PortForwardEventType};
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, Mutex};

use crate::server::{RunningServer, VsockServer};

const PORT_FORWARD_RANGE_MIN: u32 = 1000;
const PORT_FORWARD_RANGE_MAX: u32 = 9999;
const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct PortForwardManager {
    tx: broadcast::Sender<PortForwardEvent>,
    snapshot: Arc<Mutex<BTreeMap<u32, u32>>>,
}

impl PortForwardManager {
    pub fn spawn() -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, _) = broadcast::channel(1024);
        let manager = Self {
            tx,
            snapshot: Arc::new(Mutex::new(BTreeMap::new())),
        };

        let task_manager = manager.clone();
        let task = tokio::spawn(async move {
            task_manager.run().await;
        });

        (manager, task)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PortForwardEvent> {
        self.tx.subscribe()
    }

    pub async fn snapshot_events(&self) -> Vec<PortForwardEvent> {
        let snapshot = self.snapshot.lock().await;
        snapshot
            .iter()
            .map(|(guest_port, vsock_port)| PortForwardEvent {
                event_type: PortForwardEventType::Added as i32,
                forward: Some(PortForward {
                    guest_port: *guest_port,
                    vsock_port: *vsock_port,
                }),
            })
            .collect()
    }

    async fn run(&self) {
        let baseline_ports = match discover_listening_ports() {
            Ok(ports) => ports,
            Err(err) => {
                tracing::warn!(error = %err, "failed to discover initial listening ports");
                BTreeSet::new()
            }
        };

        tracing::info!(
            excluded_baseline_ports = baseline_ports.len(),
            range_start = PORT_FORWARD_RANGE_MIN,
            range_end = PORT_FORWARD_RANGE_MAX,
            "port-forward polling initialized"
        );

        let mut known_dynamic_ports = BTreeSet::new();
        let mut running = BTreeMap::<u32, RunningServer>::new();
        let mut tick = tokio::time::interval(POLL_INTERVAL);

        loop {
            tick.tick().await;

            let current_ports = match discover_listening_ports() {
                Ok(ports) => ports,
                Err(err) => {
                    tracing::warn!(error = %err, "failed to discover listening ports");
                    continue;
                }
            };

            let current_dynamic = current_ports
                .difference(&baseline_ports)
                .copied()
                .collect::<BTreeSet<_>>();

            let added = current_dynamic
                .difference(&known_dynamic_ports)
                .copied()
                .collect::<Vec<_>>();
            let removed = known_dynamic_ports
                .difference(&current_dynamic)
                .copied()
                .collect::<Vec<_>>();

            for guest_port in added {
                tracing::info!(guest_port, "detected newly listening guest tcp port");
                match start_guest_port_forwarder(guest_port) {
                    Ok(server) => {
                        let vsock_port = server.port;
                        running.insert(guest_port, server);
                        known_dynamic_ports.insert(guest_port);
                        self.snapshot.lock().await.insert(guest_port, vsock_port);
                        tracing::info!(guest_port, vsock_port, "started guest port forwarder");
                        self.publish(PortForwardEvent {
                            event_type: PortForwardEventType::Added as i32,
                            forward: Some(PortForward {
                                guest_port,
                                vsock_port,
                            }),
                        });
                        tracing::info!(
                            guest_port,
                            vsock_port,
                            "published port-forward added event"
                        );
                    }
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            guest_port,
                            "failed to start guest port forwarder"
                        );
                    }
                }
            }

            for guest_port in removed {
                if let Some(mut server) = running.remove(&guest_port) {
                    let vsock_port = server.port;
                    tracing::info!(guest_port, vsock_port, "detected guest tcp port closed");
                    server.shutdown_and_wait().await;
                    tracing::info!(guest_port, vsock_port, "guest port forwarder shut down");
                    known_dynamic_ports.remove(&guest_port);
                    self.snapshot.lock().await.remove(&guest_port);
                    self.publish(PortForwardEvent {
                        event_type: PortForwardEventType::Removed as i32,
                        forward: Some(PortForward {
                            guest_port,
                            vsock_port,
                        }),
                    });
                    tracing::info!(
                        guest_port,
                        vsock_port,
                        "published port-forward removed event"
                    );
                }
            }
        }
    }

    fn publish(&self, event: PortForwardEvent) {
        if let Err(err) = self.tx.send(event) {
            tracing::debug!(error = %err, "no active port-forward subscribers for event");
        }
    }
}

fn start_guest_port_forwarder(guest_port: u32) -> eyre::Result<RunningServer> {
    VsockServer::create(move |mut stream| async move {
        let mut target = TcpStream::connect(("127.0.0.1", guest_port as u16)).await?;
        let _ = copy_bidirectional(&mut stream, &mut target).await?;
        Ok(())
    })
    .with_concurrency(256)
    .with_tracing(tracing::info_span!(
        "vsock_server",
        service = "port-forward",
        guest_port
    ))
    .listen(None)
}

fn discover_listening_ports() -> io::Result<BTreeSet<u32>> {
    let mut ports = BTreeSet::new();
    parse_proc_net("/proc/net/tcp", &mut ports)?;
    parse_proc_net("/proc/net/tcp6", &mut ports)?;
    Ok(ports)
}

fn parse_proc_net(path: &str, ports: &mut BTreeSet<u32>) -> io::Result<()> {
    let contents = fs::read_to_string(path)?;
    for line in contents.lines().skip(1) {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 4 {
            continue;
        }

        let local_address = columns[1];
        let state = columns[3];
        if state != "0A" {
            continue;
        }

        let Some((_host, port_hex)) = local_address.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = u32::from_str_radix(port_hex, 16) else {
            continue;
        };

        if (PORT_FORWARD_RANGE_MIN..=PORT_FORWARD_RANGE_MAX).contains(&port) {
            ports.insert(port);
        }
    }

    Ok(())
}
