use bento_protocol::instance::v1::instance_control_service_server::{
    InstanceControlService, InstanceControlServiceServer,
};
use bento_protocol::instance::v1::{HealthRequest, HealthResponse};
use futures::stream;
use tokio::net::UnixStream;
use tonic::{Request, Response, Status};

#[derive(Clone, Copy)]
struct InstanceControlSvc {
    ready: bool,
}

#[tonic::async_trait]
impl InstanceControlService for InstanceControlSvc {
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let response = HealthResponse {
            ok: self.ready,
            message: if self.ready {
                String::new()
            } else {
                "service registry not ready".to_string()
            },
        };

        tracing::info!(
            service = "instance_control.health",
            ok = response.ok,
            message = %response.message,
            "instance control health request"
        );

        Ok(Response::new(response))
    }
}

pub(crate) async fn serve(stream: UnixStream, ready: bool) -> eyre::Result<()> {
    let incoming = stream::once(async move { Ok::<_, std::io::Error>(stream) });
    tonic::transport::Server::builder()
        .add_service(InstanceControlServiceServer::new(InstanceControlSvc {
            ready,
        }))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
