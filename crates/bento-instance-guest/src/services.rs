use bento_protocol::{GuestDiscovery, HealthStatus, ServiceEndpoint};
use futures::StreamExt;
use tarpc::server::{self, Channel};
use tokio_vsock::VsockStream;

#[derive(Clone)]
pub struct GuestDiscoveryService {
    pub services: Vec<ServiceEndpoint>,
}

impl GuestDiscoveryService {
    pub fn new(services: Vec<ServiceEndpoint>) -> Self {
        Self { services }
    }
}

impl GuestDiscovery for GuestDiscoveryService {
    async fn list_services(self, _: tarpc::context::Context) -> Vec<ServiceEndpoint> {
        self.services
    }

    async fn resolve_service(
        self,
        _: tarpc::context::Context,
        name: String,
    ) -> Option<ServiceEndpoint> {
        self.services
            .into_iter()
            .find(|service| service.name == name)
    }

    async fn health(self, _: tarpc::context::Context) -> HealthStatus {
        HealthStatus { ok: true }
    }
}

pub async fn serve_discovery_connection(
    stream: VsockStream,
    discovery: GuestDiscoveryService,
) -> eyre::Result<()> {
    let framed = tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec::builder()
        .new_framed(stream);

    let transport =
        tarpc::serde_transport::new(framed, tarpc::tokio_serde::formats::Bincode::default());

    server::BaseChannel::with_defaults(transport)
        .execute(discovery.serve())
        .for_each(|response| async move {
            tokio::spawn(response);
        })
        .await;

    Ok(())
}
