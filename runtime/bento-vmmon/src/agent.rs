use std::io;
use std::sync::Arc;
use std::time::Duration;

use bento_protocol::v1::agent_service_client::AgentServiceClient;
use bento_protocol::v1::{AgentPingRequest, HealthRequest, HealthResponse};
use bento_virt::VirtualMachine;
use eyre::Context;
use futures::stream::{self, Stream};
use hyper_util::rt::TokioIo;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

const AGENT_PROBE_RETRY: Duration = Duration::from_secs(1);
pub(crate) const AGENT_CONTROL_PORT: u32 = 1027;

pub(crate) struct AgentClient {
    machine: VirtualMachine,
    port: u32,
    client: Option<AgentServiceClient<Channel>>,
}

impl AgentClient {
    pub(crate) fn new(machine: &VirtualMachine) -> Self {
        Self {
            machine: machine.clone(),
            port: AGENT_CONTROL_PORT,
            client: None,
        }
    }

    pub(crate) async fn health(&mut self) -> eyre::Result<HealthResponse> {
        let client = self.connect().await?;
        client
            .health(HealthRequest {})
            .await
            .map(|response| response.into_inner())
            .context("agent health failed")
    }

    pub(crate) fn watch(
        self,
        shutdown: CancellationToken,
    ) -> impl Stream<Item = eyre::Result<HealthResponse>> {
        let interval = tokio::time::interval(AGENT_PROBE_RETRY);

        stream::unfold(
            (self, shutdown, interval),
            |(mut client, shutdown, mut interval)| async move {
                tokio::select! {
                    _ = shutdown.cancelled() => None,
                    _ = interval.tick() => Some((client.health().await, (client, shutdown, interval))),
                }
            },
        )
    }

    async fn connect(&mut self) -> eyre::Result<&mut AgentServiceClient<Channel>> {
        if let Some(mut client) = self.client.take() {
            if Self::ping(&mut client).await.is_ok() {
                self.client = Some(client);
                return self.client.as_mut().ok_or_else(|| {
                    eyre::eyre!("agent client cache was empty after successful ping")
                });
            }
        }

        self.client = Some(connect_agent_client(&self.machine, self.port).await?);

        self.client
            .as_mut()
            .ok_or_else(|| eyre::eyre!("agent client cache was empty after connect"))
    }

    async fn ping(client: &mut AgentServiceClient<Channel>) -> eyre::Result<()> {
        client
            .ping(AgentPingRequest {
                message: String::new(),
            })
            .await
            .context("agent ping failed")?;
        Ok(())
    }
}

async fn connect_agent_client(
    machine: &VirtualMachine,
    port: u32,
) -> eyre::Result<AgentServiceClient<Channel>> {
    let stream = machine.connect_vsock(port).await?;
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
                        "agent connector stream already consumed",
                    )
                })
                .map(TokioIo::new)
        }
    });

    let channel = Endpoint::from_static("http://agent.local")
        .connect_with_connector(connector)
        .await
        .context("connect agent rpc client")?;

    Ok(AgentServiceClient::new(channel))
}

#[cfg(test)]
mod tests {
    use super::AGENT_CONTROL_PORT;

    #[test]
    fn agent_control_port_is_fixed() {
        assert_eq!(AGENT_CONTROL_PORT, 1027);
    }
}
