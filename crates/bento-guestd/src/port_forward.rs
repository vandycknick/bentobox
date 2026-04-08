use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bento_core::capabilities::{ForwardCapabilityConfig, UdsForwardConfig};
use bento_protocol::v1::{EndpointDescriptor, EndpointKind, Port, PortEvent, PortEventType};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::{broadcast, Mutex};

use crate::server::{RunningServer, VsockServer};

const PORT_FORWARD_RANGE_MIN: u32 = 1000;
const PORT_FORWARD_RANGE_MAX: u32 = 9999;
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct ForwardRuntime {
    endpoints: Vec<EndpointDescriptor>,
    port_manager: Option<PortForwardManager>,
    _port_task: Option<tokio::task::JoinHandle<()>>,
    _uds_servers: Vec<RunningServer>,
}

impl ForwardRuntime {
    pub fn disabled() -> Self {
        Self {
            endpoints: Vec::new(),
            port_manager: None,
            _port_task: None,
            _uds_servers: Vec::new(),
        }
    }

    pub fn start(config: &ForwardCapabilityConfig) -> eyre::Result<Self> {
        if !config.enabled {
            return Ok(Self::disabled());
        }

        let mut endpoints = Vec::new();
        let mut uds_servers = Vec::new();

        for forward in &config.uds {
            let server = start_guest_uds_forwarder(forward.clone())?;
            endpoints.push(EndpointDescriptor {
                name: forward.name.clone(),
                kind: EndpointKind::UnixSocket as i32,
                port: server.port,
                guest_address: forward.guest_path.clone(),
            });
            uds_servers.push(server);
        }

        let (port_manager, port_task) = if config.tcp.auto_discover {
            let (manager, task) = PortForwardManager::spawn();
            (Some(manager), Some(task))
        } else {
            (None, None)
        };

        Ok(Self {
            endpoints,
            port_manager,
            _port_task: port_task,
            _uds_servers: uds_servers,
        })
    }

    pub fn endpoints(&self) -> &[EndpointDescriptor] {
        &self.endpoints
    }

    pub fn port_manager(&self) -> Option<&PortForwardManager> {
        self.port_manager.as_ref()
    }
}

#[derive(Clone)]
pub struct PortForwardManager {
    tx: broadcast::Sender<PortEvent>,
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

    pub fn subscribe(&self) -> broadcast::Receiver<PortEvent> {
        self.tx.subscribe()
    }

    pub async fn snapshot_events(&self) -> Vec<PortEvent> {
        let snapshot = self.snapshot.lock().await;
        snapshot
            .iter()
            .map(|(guest_port, vsock_port)| PortEvent {
                event_type: PortEventType::Added as i32,
                forward: Some(Port {
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
            "tcp forward polling initialized"
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
                match start_guest_tcp_forwarder(guest_port) {
                    Ok(server) => {
                        let vsock_port = server.port;
                        running.insert(guest_port, server);
                        known_dynamic_ports.insert(guest_port);
                        self.snapshot.lock().await.insert(guest_port, vsock_port);
                        self.publish(PortEvent {
                            event_type: PortEventType::Added as i32,
                            forward: Some(Port {
                                guest_port,
                                vsock_port,
                            }),
                        });
                    }
                    Err(err) => {
                        tracing::error!(error = %err, guest_port, "failed to start tcp forwarder");
                    }
                }
            }

            for guest_port in removed {
                if let Some(mut server) = running.remove(&guest_port) {
                    let vsock_port = server.port;
                    server.shutdown_and_wait().await;
                    known_dynamic_ports.remove(&guest_port);
                    self.snapshot.lock().await.remove(&guest_port);
                    self.publish(PortEvent {
                        event_type: PortEventType::Removed as i32,
                        forward: Some(Port {
                            guest_port,
                            vsock_port,
                        }),
                    });
                }
            }
        }
    }

    fn publish(&self, event: PortEvent) {
        if let Err(err) = self.tx.send(event) {
            tracing::debug!(error = %err, "no active tcp forward subscribers for event");
        }
    }
}

fn start_guest_tcp_forwarder(guest_port: u32) -> eyre::Result<RunningServer> {
    VsockServer::create(move |mut stream| async move {
        let mut target = TcpStream::connect(("127.0.0.1", guest_port as u16)).await?;
        let _ = copy_bidirectional(&mut stream, &mut target).await?;
        Ok(())
    })
    .with_concurrency(256)
    .with_tracing(tracing::info_span!(
        "vsock_server",
        capability = "forward",
        guest_port
    ))
    .listen(None)
}

fn start_guest_uds_forwarder(forward: UdsForwardConfig) -> eyre::Result<RunningServer> {
    let guest_path = forward.guest_path.clone();
    let name = forward.name.clone();

    VsockServer::create(move |mut stream| {
        let guest_path = guest_path.clone();
        async move {
            let mut target = UnixStream::connect(&guest_path).await?;
            let _ = copy_bidirectional(&mut stream, &mut target).await?;
            Ok(())
        }
    })
    .with_concurrency(256)
    .with_tracing(tracing::info_span!("vsock_server", capability = "forward", endpoint = %name))
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
