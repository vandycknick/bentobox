use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bento_protocol::v1::agent_service_client::AgentServiceClient;
use bento_protocol::v1::{AgentPingRequest, CapabilityStatus, Empty, EndpointDescriptor};
use bento_protocol::DEFAULT_DISCOVERY_PORT;
use bento_runtime::profiles::ENDPOINT_SERIAL;
use bento_vmm::VirtualMachine;
use eyre::Context;
use hyper_util::rt::TokioIo;
use tokio::sync::Mutex;
use tonic::transport::Endpoint;
use tower::service_fn;

#[derive(Debug, Clone, Copy)]
pub(crate) enum ServiceTarget {
    VsockPort(u32),
    Serial,
}

#[derive(Debug)]
pub(crate) struct ServiceRegistry {
    by_name: BTreeMap<String, ServiceTarget>,
    capabilities: BTreeMap<String, CapabilityStatus>,
    endpoints: BTreeMap<String, EndpointDescriptor>,
}

impl ServiceRegistry {
    pub(crate) async fn discover(machine: &VirtualMachine) -> eyre::Result<Self> {
        let mut by_name = BTreeMap::new();
        by_name.insert(ENDPOINT_SERIAL.to_string(), ServiceTarget::Serial);

        let mut client = connect_guest_client(machine).await?;
        verify_guest_liveness(&mut client).await?;

        let endpoints =
            tokio::time::timeout(Duration::from_secs(3), client.list_endpoints(Empty {}))
                .await
                .map_err(|_| eyre::eyre!("guest agent list_endpoints request timed out"))?
                .map_err(|err| eyre::eyre!("query guest endpoint list failed: {err}"))?
                .into_inner()
                .endpoints;

        let endpoint_map = endpoints
            .into_iter()
            .map(|endpoint| {
                by_name.insert(
                    endpoint.name.clone(),
                    ServiceTarget::VsockPort(endpoint.port),
                );
                (endpoint.name.clone(), endpoint)
            })
            .collect();

        let capabilities =
            tokio::time::timeout(Duration::from_secs(3), client.list_capabilities(Empty {}))
                .await
                .map_err(|_| eyre::eyre!("guest agent list_capabilities request timed out"))?
                .map_err(|err| eyre::eyre!("query guest capability list failed: {err}"))?
                .into_inner();

        let capabilities = capabilities
            .capabilities
            .into_iter()
            .map(|capability| (capability.name.clone(), capability))
            .collect();

        Ok(Self {
            by_name,
            capabilities,
            endpoints: endpoint_map,
        })
    }

    pub(crate) fn resolve(&self, name: &str) -> Option<ServiceTarget> {
        self.by_name.get(name).copied()
    }

    pub(crate) fn capabilities(&self) -> impl Iterator<Item = &CapabilityStatus> {
        self.capabilities.values()
    }

    pub(crate) fn endpoints(&self) -> impl Iterator<Item = &EndpointDescriptor> {
        self.endpoints.values()
    }
}

async fn connect_guest_client(
    machine: &VirtualMachine,
) -> eyre::Result<AgentServiceClient<tonic::transport::Channel>> {
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

    Ok(AgentServiceClient::new(channel))
}

async fn verify_guest_liveness(
    client: &mut AgentServiceClient<tonic::transport::Channel>,
) -> eyre::Result<()> {
    let pong = tokio::time::timeout(
        Duration::from_secs(3),
        client.ping(AgentPingRequest {
            message: String::from("ping"),
        }),
    )
    .await
    .map_err(|_| eyre::eyre!("guest agent ping request timed out"))?
    .map_err(|err| eyre::eyre!("query guest agent ping failed: {err}"))?
    .into_inner();

    if pong.message != "pong" {
        eyre::bail!(
            "guest agent returned unexpected ping response: {}",
            pong.message
        );
    }

    Ok(())
}
