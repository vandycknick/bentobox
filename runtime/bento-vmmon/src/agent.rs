use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bento_protocol::v1::agent_control_service_server::{
    AgentControlService, AgentControlServiceServer,
};
use bento_protocol::v1::{
    GetAgentConfigRequest, GetAgentConfigResponse, RegisterAgentRequest, RegisterAgentResponse,
};
use bento_virt::{VirtualMachine, VsockListener, VsockStream};
use eyre::Context as EyreContext;
use futures::stream::{self, Stream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};

use crate::state::{Action, InstanceStore};

pub(crate) const AGENT_CONTROL_PORT: u32 = 1027;

#[derive(Clone)]
struct AgentControlSvc {
    store: Arc<InstanceStore>,
    config: Arc<String>,
    ready: Arc<AtomicBool>,
}

impl AgentControlSvc {
    fn new(store: Arc<InstanceStore>, config: String, ready: Arc<AtomicBool>) -> Self {
        Self {
            store,
            config: Arc::new(config),
            ready,
        }
    }
}

#[tonic::async_trait]
impl AgentControlService for AgentControlSvc {
    async fn register(
        &self,
        request: Request<RegisterAgentRequest>,
    ) -> Result<Response<RegisterAgentResponse>, Status> {
        let request = request.into_inner();
        let hostname = request
            .system_info
            .as_ref()
            .map(|system| system.hostname.as_str())
            .unwrap_or("");
        let arch = request
            .system_info
            .as_ref()
            .map(|system| system.arch.as_str())
            .unwrap_or("");

        tracing::info!(
            agent_version = %request.agent_version,
            hostname,
            arch,
            "guest agent registered"
        );
        self.ready.store(true, Ordering::Release);
        self.store.dispatch(Action::guest_running());

        Ok(Response::new(RegisterAgentResponse {
            accepted: true,
            message: String::from("registered"),
        }))
    }

    async fn get_config(
        &self,
        _request: Request<GetAgentConfigRequest>,
    ) -> Result<Response<GetAgentConfigResponse>, Status> {
        tracing::info!(
            config_len = self.config.len(),
            "guest agent config requested"
        );
        Ok(Response::new(GetAgentConfigResponse {
            config: self.config.as_ref().clone(),
        }))
    }
}

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

pub(crate) async fn spawn_agent_control_service(
    machine: &VirtualMachine,
    store: Arc<InstanceStore>,
    config: String,
    timeout: Duration,
    shutdown: CancellationToken,
) -> eyre::Result<JoinHandle<()>> {
    let listener = machine
        .listen_vsock(AGENT_CONTROL_PORT)
        .await
        .context("listen for guest agent control connections")?;
    let ready = Arc::new(AtomicBool::new(false));
    let service = AgentControlSvc::new(store.clone(), config, ready.clone());
    let timeout_task = spawn_readiness_timeout(store.clone(), ready, timeout, shutdown.clone());

    Ok(tokio::spawn(async move {
        let result = serve_agent_control(listener, service, shutdown).await;
        timeout_task.abort();

        if let Err(err) = result {
            tracing::warn!(error = %err, "agent control service failed");
            store.dispatch(Action::guest_error(format!(
                "agent control service failed: {err}"
            )));
        }
    }))
}

async fn serve_agent_control(
    listener: VsockListener,
    service: AgentControlSvc,
    shutdown: CancellationToken,
) -> eyre::Result<()> {
    tonic::transport::Server::builder()
        .add_service(AgentControlServiceServer::new(service))
        .serve_with_incoming_shutdown(incoming_vsock_connections(listener), shutdown.cancelled())
        .await?;
    Ok(())
}

fn incoming_vsock_connections(
    listener: VsockListener,
) -> impl Stream<Item = io::Result<ConnectedVsock>> {
    stream::unfold(listener, |mut listener| async move {
        let accepted = listener.accept().await.map(ConnectedVsock);
        Some((accepted, listener))
    })
}

fn spawn_readiness_timeout(
    store: Arc<InstanceStore>,
    ready: Arc<AtomicBool>,
    timeout: Duration,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = tokio::time::sleep(timeout) => {
                if !ready.load(Ordering::Acquire) {
                    tracing::warn!(timeout = ?timeout, "guest agent did not register before timeout");
                    store.dispatch(Action::guest_error(format!(
                        "guest agent did not register within {} seconds",
                        timeout.as_secs()
                    )));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::AGENT_CONTROL_PORT;

    #[test]
    fn agent_control_port_is_fixed() {
        assert_eq!(AGENT_CONTROL_PORT, 1027);
    }
}
