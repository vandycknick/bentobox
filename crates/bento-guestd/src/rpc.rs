use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bento_protocol::v1::agent_service_server::{AgentService, AgentServiceServer};
use bento_protocol::v1::{
    AgentPingRequest, AgentPingResponse, CapabilityStatus, Empty, EndpointDescriptor,
    ListCapabilitiesResponse, ListEndpointsResponse, PortEvent, SystemInfo, WatchPortsRequest,
};
use bento_runtime::capabilities::{
    CapabilitiesConfig, CAPABILITY_DNS, CAPABILITY_FORWARD, CAPABILITY_SSH,
};
use futures::stream::{self, Stream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_vsock::VsockStream;
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};

use crate::port_forward::ForwardRuntime;
use crate::system_info::collect_system_info;

#[derive(Debug)]
struct ConnectedVsock(VsockStream);

impl Connected for ConnectedVsock {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncRead for ConnectedVsock {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for ConnectedVsock {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[derive(Clone)]
pub struct AgentContext {
    capabilities: CapabilitiesConfig,
    endpoints: Arc<Vec<EndpointDescriptor>>,
    forward: Arc<ForwardRuntime>,
}

impl AgentContext {
    pub fn new(
        capabilities: CapabilitiesConfig,
        endpoints: Vec<EndpointDescriptor>,
        forward: ForwardRuntime,
    ) -> Self {
        Self {
            capabilities,
            endpoints: Arc::new(endpoints),
            forward: Arc::new(forward),
        }
    }
}

type WatchPortsStream = Pin<Box<dyn Stream<Item = Result<PortEvent, Status>> + Send>>;

#[tonic::async_trait]
impl AgentService for AgentContext {
    type WatchPortsStream = WatchPortsStream;

    async fn ping(
        &self,
        _request: Request<AgentPingRequest>,
    ) -> Result<Response<AgentPingResponse>, Status> {
        Ok(Response::new(AgentPingResponse {
            message: String::from("pong"),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    async fn get_system_info(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<SystemInfo>, Status> {
        let info = collect_system_info().map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(info))
    }

    async fn list_capabilities(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListCapabilitiesResponse>, Status> {
        Ok(Response::new(ListCapabilitiesResponse {
            capabilities: capability_statuses(&self.capabilities).await,
        }))
    }

    async fn list_endpoints(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListEndpointsResponse>, Status> {
        Ok(Response::new(ListEndpointsResponse {
            endpoints: self.endpoints.as_ref().clone(),
        }))
    }

    async fn watch_ports(
        &self,
        _request: Request<WatchPortsRequest>,
    ) -> Result<Response<Self::WatchPortsStream>, Status> {
        let Some(manager) = self.forward.port_manager() else {
            return Ok(Response::new(Box::pin(stream::empty())));
        };

        let snapshot = manager.snapshot_events().await;
        let rx = manager.subscribe();
        let pending = VecDeque::from(snapshot);
        let updates = stream::unfold((pending, rx), |(mut pending, mut rx)| async move {
            if let Some(event) = pending.pop_front() {
                return Some((Ok(event), (pending, rx)));
            }

            match rx.recv().await {
                Ok(update) => Some((Ok(update), (pending, rx))),
                Err(broadcast::error::RecvError::Lagged(skipped)) => Some((
                    Err(Status::resource_exhausted(format!(
                        "port stream lagged, skipped {skipped} updates"
                    ))),
                    (pending, rx),
                )),
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });

        Ok(Response::new(Box::pin(updates)))
    }
}

async fn capability_statuses(capabilities: &CapabilitiesConfig) -> Vec<CapabilityStatus> {
    let mut statuses = Vec::new();

    if capabilities.ssh.enabled {
        statuses.push(probe_ssh().await);
    }

    if capabilities.dns.enabled {
        statuses.push(CapabilityStatus {
            name: String::from(CAPABILITY_DNS),
            enabled: true,
            startup_required: false,
            configured: true,
            running: true,
            summary: String::from("DNS capability configured"),
            problems: Vec::new(),
        });
    }

    if capabilities.forward.enabled {
        statuses.push(CapabilityStatus {
            name: String::from(CAPABILITY_FORWARD),
            enabled: true,
            startup_required: false,
            configured: true,
            running: true,
            summary: String::from("Forward capability configured"),
            problems: Vec::new(),
        });
    }

    statuses
}

async fn probe_ssh() -> CapabilityStatus {
    let configured =
        command_exists("sshd") || std::path::Path::new("/etc/ssh/sshd_config").exists();
    let running = TcpStream::connect("127.0.0.1:22").await.is_ok();

    let (summary, problems) = match (configured, running) {
        (true, true) => (
            String::from("OpenSSH is configured and running"),
            Vec::new(),
        ),
        (true, false) => (
            String::from("OpenSSH is configured but not running"),
            vec![String::from("OpenSSH service is not reachable")],
        ),
        (false, _) => (
            String::from("OpenSSH is not configured"),
            vec![String::from(
                "OpenSSH is not installed or configured in the guest",
            )],
        ),
    };

    CapabilityStatus {
        name: String::from(CAPABILITY_SSH),
        enabled: true,
        startup_required: true,
        configured,
        running,
        summary,
        problems,
    }
}

fn command_exists(command: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {command} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub async fn serve_agent_connection(stream: VsockStream, agent: AgentContext) -> eyre::Result<()> {
    let incoming = stream::once(async move { Ok::<_, io::Error>(ConnectedVsock(stream)) });

    tonic::transport::Server::builder()
        .add_service(AgentServiceServer::new(agent))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
