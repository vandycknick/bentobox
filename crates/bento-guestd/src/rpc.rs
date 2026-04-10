use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bento_core::services::{GuestRuntimeConfig, GuestServiceConfig, GuestServiceKind};
use bento_protocol::v1::agent_service_server::{AgentService, AgentServiceServer};
use bento_protocol::v1::{
    AgentPingRequest, AgentPingResponse, Empty, HealthRequest, HealthResponse, ServiceHealth,
    SystemInfo,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UnixStream};
use tokio_vsock::VsockStream;
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};

use crate::host::info::get_system_info;

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
    config: Arc<GuestRuntimeConfig>,
}

impl AgentContext {
    pub fn new(config: GuestRuntimeConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

#[tonic::async_trait]
impl AgentService for AgentContext {
    async fn ping(
        &self,
        _request: Request<AgentPingRequest>,
    ) -> Result<Response<AgentPingResponse>, Status> {
        Ok(Response::new(AgentPingResponse {
            message: String::from("pong"),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let services = service_health(&self.config).await;
        let ready = services
            .iter()
            .filter(|service| service.startup_required)
            .all(|service| service.healthy);
        let summary = health_summary(&services, ready);

        Ok(Response::new(HealthResponse {
            ready,
            summary,
            services,
        }))
    }

    async fn get_system_info(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<SystemInfo>, Status> {
        let system_info = get_system_info()
            .map_err(|err| Status::internal(format!("failed to collect system info: {err}")))?;
        Ok(Response::new(system_info))
    }
}

async fn service_health(config: &GuestRuntimeConfig) -> Vec<ServiceHealth> {
    let mut statuses = Vec::new();

    if config.dns.enabled {
        let (healthy, summary, problems) = dns_health(config).await;
        statuses.push(ServiceHealth {
            name: String::from("dns"),
            startup_required: false,
            healthy,
            summary,
            problems,
        });
    }

    for service in &config.services {
        statuses.push(probe_service(service).await);
    }

    statuses
}

async fn dns_health(config: &GuestRuntimeConfig) -> (bool, String, Vec<String>) {
    let listen = config.dns.listen_address;
    let configured = std::path::Path::new("/etc/resolv.conf").exists();

    if configured {
        (true, format!("dns configured for {}", listen), Vec::new())
    } else {
        (
            false,
            String::from("dns configuration is missing"),
            vec![String::from("/etc/resolv.conf was not configured")],
        )
    }
}

async fn probe_service(service: &GuestServiceConfig) -> ServiceHealth {
    match service.kind {
        GuestServiceKind::Shell => probe_shell(service).await,
        GuestServiceKind::UnixSocketForward => probe_unix_socket_forward(service).await,
    }
}

async fn probe_shell(service: &GuestServiceConfig) -> ServiceHealth {
    let healthy = TcpStream::connect("127.0.0.1:22").await.is_ok();
    let (summary, problems) = if healthy {
        (String::from("shell is reachable"), Vec::new())
    } else {
        (
            String::from("shell is not reachable"),
            vec![String::from("failed to connect to 127.0.0.1:22")],
        )
    };

    ServiceHealth {
        name: service.id.clone(),
        startup_required: service.startup_required,
        healthy,
        summary,
        problems,
    }
}

async fn probe_unix_socket_forward(service: &GuestServiceConfig) -> ServiceHealth {
    let healthy = UnixStream::connect(&service.target).await.is_ok();
    let (summary, problems) = if healthy {
        (format!("{} is reachable", service.target), Vec::new())
    } else {
        (
            format!("{} is not reachable", service.target),
            vec![format!("failed to connect to {}", service.target)],
        )
    };

    ServiceHealth {
        name: service.id.clone(),
        startup_required: service.startup_required,
        healthy,
        summary,
        problems,
    }
}

fn health_summary(services: &[ServiceHealth], ready: bool) -> String {
    if ready {
        return String::from("startup-required guest services are healthy");
    }

    let waiting = services
        .iter()
        .filter(|service| service.startup_required && !service.healthy)
        .map(|service| {
            let detail = service
                .problems
                .first()
                .cloned()
                .unwrap_or_else(|| service.summary.clone());
            format!("{}: {}", service.name, detail)
        })
        .collect::<Vec<_>>();

    if waiting.is_empty() {
        String::from("guest services are starting")
    } else {
        waiting.join("; ")
    }
}

pub async fn serve_agent_connection(stream: VsockStream, agent: AgentContext) -> eyre::Result<()> {
    let incoming = futures::stream::once(async move { Ok::<_, io::Error>(ConnectedVsock(stream)) });

    tonic::transport::Server::builder()
        .add_service(AgentServiceServer::new(agent))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
