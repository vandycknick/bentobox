use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::config::MountConfig;
use bento_protocol::guest::v1::guest_discovery_service_server::{
    GuestDiscoveryService, GuestDiscoveryServiceServer,
};
use bento_protocol::guest::v1::{
    ExtensionStatus, HealthRequest, HealthResponse, ListExtensionsRequest, ListExtensionsResponse,
    ListServicesRequest, ListServicesResponse, ResolveServiceRequest, ResolveServiceResponse,
    ServiceEndpoint, ServiceHealth, ServiceStatus,
};
use bento_runtime::extensions::{BuiltinExtension, ExtensionsConfig};
use futures::stream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UnixStream};
use tokio_vsock::VsockStream;
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};

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
pub struct GuestDiscoveryState {
    services: Arc<Vec<ServiceEndpoint>>,
    extensions: ExtensionsConfig,
    _mounts: Arc<Vec<MountConfig>>,
}

impl GuestDiscoveryState {
    pub fn new(
        services: Vec<ServiceEndpoint>,
        extensions: ExtensionsConfig,
        mounts: Vec<MountConfig>,
    ) -> Self {
        Self {
            services: Arc::new(services),
            extensions,
            _mounts: Arc::new(mounts),
        }
    }
}

#[tonic::async_trait]
impl GuestDiscoveryService for GuestDiscoveryState {
    async fn list_services(
        &self,
        _request: Request<ListServicesRequest>,
    ) -> Result<Response<ListServicesResponse>, Status> {
        Ok(Response::new(ListServicesResponse {
            services: self.services.as_ref().clone(),
        }))
    }

    async fn resolve_service(
        &self,
        request: Request<ResolveServiceRequest>,
    ) -> Result<Response<ResolveServiceResponse>, Status> {
        let name = request.into_inner().name;
        let service = self
            .services
            .iter()
            .find(|service| service.name == name)
            .cloned();

        Ok(Response::new(ResolveServiceResponse { service }))
    }

    async fn list_extensions(
        &self,
        _request: Request<ListExtensionsRequest>,
    ) -> Result<Response<ListExtensionsResponse>, Status> {
        Ok(Response::new(ListExtensionsResponse {
            extensions: extension_statuses(&self.extensions).await,
        }))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let statuses: Vec<ServiceHealth> = self
            .services
            .iter()
            .map(|service| ServiceHealth {
                name: service.name.clone(),
                status: ServiceStatus::Running as i32,
                message: String::new(),
            })
            .collect();

        Ok(Response::new(HealthResponse {
            ok: statuses
                .iter()
                .all(|status| status.status == ServiceStatus::Running as i32),
            services: statuses,
        }))
    }
}

async fn extension_statuses(extensions: &ExtensionsConfig) -> Vec<ExtensionStatus> {
    let mut statuses = Vec::new();
    if extensions.is_enabled(BuiltinExtension::Ssh) {
        statuses.push(probe_ssh().await);
    }
    if extensions.is_enabled(BuiltinExtension::Docker) {
        statuses.push(probe_docker().await);
    }
    statuses
}

async fn probe_ssh() -> ExtensionStatus {
    let configured =
        command_exists("sshd") || std::path::Path::new("/etc/ssh/sshd_config").exists();
    let running = TcpStream::connect("127.0.0.1:22").await.is_ok();
    extension_status("ssh", true, configured, running, "OpenSSH")
}

async fn probe_docker() -> ExtensionStatus {
    let configured = command_exists("dockerd") || command_exists("docker");
    let running = UnixStream::connect("/var/run/docker.sock").await.is_ok();
    extension_status("docker", true, configured, running, "Docker")
}

fn extension_status(
    name: &str,
    enabled: bool,
    configured: bool,
    running: bool,
    label: &str,
) -> ExtensionStatus {
    let (summary, problems) = match (configured, running) {
        (true, true) => (format!("{label} is configured and running"), Vec::new()),
        (true, false) => (
            format!("{label} is configured but not running"),
            vec![format!("{label} service is not reachable")],
        ),
        (false, _) => (
            format!("{label} is not configured"),
            vec![format!(
                "{label} is not installed or configured in the guest"
            )],
        ),
    };

    ExtensionStatus {
        name: name.to_string(),
        enabled,
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

pub async fn serve_discovery_connection(
    stream: VsockStream,
    discovery: GuestDiscoveryState,
) -> eyre::Result<()> {
    let incoming = stream::once(async move { Ok::<_, io::Error>(ConnectedVsock(stream)) });

    tonic::transport::Server::builder()
        .add_service(GuestDiscoveryServiceServer::new(discovery))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
