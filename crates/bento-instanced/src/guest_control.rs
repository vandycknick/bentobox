use std::io;
use std::sync::Arc;

use bento_protocol::v1::agent_service_client::AgentServiceClient;
use bento_protocol::v1::{AgentPingRequest, HealthRequest, HealthResponse};
use bento_protocol::DEFAULT_AGENT_CONTROL_PORT;
use bento_vmm::VirtualMachine;
use eyre::Context;
use hyper_util::rt::TokioIo;
use tokio::sync::Mutex;
use tonic::transport::Endpoint;
use tower::service_fn;

pub(crate) async fn health(machine: &VirtualMachine) -> eyre::Result<HealthResponse> {
    let mut client = connect_guest_client(machine).await?;
    client
        .ping(AgentPingRequest {
            message: String::new(),
        })
        .await?;
    Ok(client.health(HealthRequest {}).await?.into_inner())
}

async fn connect_guest_client(
    machine: &VirtualMachine,
) -> eyre::Result<AgentServiceClient<tonic::transport::Channel>> {
    let stream = machine.connect_vsock(DEFAULT_AGENT_CONTROL_PORT).await?;
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
                        "guest control connector stream already consumed",
                    )
                })
                .map(TokioIo::new)
        }
    });

    let channel = Endpoint::from_static("http://guest-control.local")
        .connect_with_connector(connector)
        .await
        .context("connect guest control rpc client")?;

    Ok(AgentServiceClient::new(channel))
}
