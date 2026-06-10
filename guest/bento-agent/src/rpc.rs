use std::io;
use std::sync::Arc;

use bento_protocol::v1::agent_control_service_client::AgentControlServiceClient;
use bento_protocol::v1::{GetAgentConfigRequest, GetAgentConfigResponse, RegisterAgentRequest};
use eyre::Context;
use hyper_util::rt::TokioIo;
use tokio::sync::Mutex;
use tokio_vsock::{VsockAddr, VsockStream, VMADDR_CID_HOST};
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

use crate::host::info::get_system_info;

pub(crate) struct AgentControlClient {
    client: AgentControlServiceClient<Channel>,
}

impl AgentControlClient {
    pub(crate) async fn connect(port: u32) -> eyre::Result<Self> {
        Ok(Self {
            client: connect_agent_control_client(port).await?,
        })
    }

    pub(crate) async fn register(&mut self) -> eyre::Result<()> {
        let system_info = get_system_info().context("collect system info for agent register")?;
        let response = self
            .client
            .register(RegisterAgentRequest {
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
                system_info: Some(system_info),
            })
            .await
            .context("register guest agent")?
            .into_inner();

        if !response.accepted {
            eyre::bail!("agent registration rejected: {}", response.message);
        }

        Ok(())
    }

    pub(crate) async fn get_config(&mut self) -> eyre::Result<GetAgentConfigResponse> {
        self.client
            .get_config(GetAgentConfigRequest {})
            .await
            .map(|response| response.into_inner())
            .context("fetch guest agent config")
    }
}

async fn connect_agent_control_client(
    port: u32,
) -> eyre::Result<AgentControlServiceClient<Channel>> {
    let stream = VsockStream::connect(VsockAddr::new(VMADDR_CID_HOST, port)).await?;
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
                        "agent control connector stream already consumed",
                    )
                })
                .map(TokioIo::new)
        }
    });

    let channel = Endpoint::from_static("http://agent-control.local")
        .connect_with_connector(connector)
        .await
        .context("connect agent control rpc client")?;

    Ok(AgentControlServiceClient::new(channel))
}
