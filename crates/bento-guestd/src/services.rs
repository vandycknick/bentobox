use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bento_protocol::guest::v1::guest_discovery_service_server::{
    GuestDiscoveryService, GuestDiscoveryServiceServer,
};
use bento_protocol::guest::v1::{
    HealthRequest, HealthResponse, ListServicesRequest, ListServicesResponse,
    ResolveServiceRequest, ResolveServiceResponse, ServiceEndpoint, ServiceHealth, ServiceStatus,
};
use futures::stream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
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
}

impl GuestDiscoveryState {
    pub fn new(services: Vec<ServiceEndpoint>) -> Self {
        Self {
            services: Arc::new(services),
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
