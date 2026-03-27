mod config;
mod init;
mod port;
mod port_forward;
mod rpc;
mod server;
mod system_info;

use std::io;

use bento_protocol::v1::{EndpointDescriptor, EndpointKind};
use bento_runtime::profiles::ENDPOINT_SSH;
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;

use crate::config::GuestdConfig;
use crate::init::ensure_supported_runtime_mode;
use crate::port_forward::ForwardRuntime;
use crate::rpc::{serve_agent_connection, AgentContext};
use crate::server::VsockServer;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> eyre::Result<()> {
    ensure_supported_runtime_mode();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .with_writer(std::io::stderr)
        .try_init();

    tracing::info!("starting");

    let guestd_config = GuestdConfig::load()?;
    let discovery_port = crate::port::from_kernel_cmdline();
    let mut endpoints = Vec::<EndpointDescriptor>::new();
    let mut running_servers = Vec::new();

    if guestd_config.capabilities.ssh.enabled {
        let ssh_server = VsockServer::create(|mut stream| async move {
            let mut ssh = TcpStream::connect("127.0.0.1:22").await?;
            let _ = copy_bidirectional(&mut stream, &mut ssh).await?;
            Ok(())
        })
        .with_concurrency(256)
        .with_tracing(tracing::info_span!("vsock_server", endpoint = ENDPOINT_SSH))
        .listen(None)?;

        endpoints.push(EndpointDescriptor {
            name: String::from(ENDPOINT_SSH),
            kind: EndpointKind::Ssh as i32,
            port: ssh_server.port,
            guest_address: String::from("127.0.0.1:22"),
        });
        running_servers.push(ssh_server);
    }

    let forward_runtime = ForwardRuntime::start(&guestd_config.capabilities.forward)?;
    endpoints.extend_from_slice(forward_runtime.endpoints());

    tracing::info!(endpoints = ?endpoints, "setting up endpoints for agent discovery");

    let agent_service = AgentContext::new(guestd_config.capabilities, endpoints, forward_runtime);

    let discovery_server = VsockServer::create(move |stream| {
        let agent = agent_service.clone();
        async move {
            serve_agent_connection(stream, agent)
                .await
                .map_err(|err| io::Error::other(err.to_string()))
        }
    })
    .with_concurrency(64)
    .with_tracing(tracing::info_span!("vsock_server", service = "agent"))
    .listen(Some(discovery_port))?;

    running_servers.push(discovery_server);

    let mut join_set = tokio::task::JoinSet::new();
    for server in running_servers {
        join_set.spawn(server.wait());
    }

    while let Some(result) = join_set.join_next().await {
        result??;
    }

    Ok(())
}
