mod port;
mod server;
mod services;

use bento_protocol::ServiceEndpoint;
use std::io;
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;

use crate::server::VsockServer;
use crate::services::{serve_discovery_connection, GuestDiscoveryService};

const SSH_SERVICE_NAME: &str = "ssh";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> eyre::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .with_writer(std::io::stderr)
        .try_init();

    tracing::info!("starting");

    let discovery_port = crate::port::from_kernel_cmdline();

    let ssh_server = VsockServer::create(|mut stream| async move {
        let mut ssh = TcpStream::connect("127.0.0.1:22").await?;
        let _ = copy_bidirectional(&mut stream, &mut ssh).await?;
        Ok(())
    })
    .with_concurrency(256)
    .with_tracing(tracing::info_span!("vsock_server", service = "ssh"))
    .listen(None)?;

    tracing::info!("setting up services for discovery: {}", "ssh");

    let discovery_service = GuestDiscoveryService::new(vec![ServiceEndpoint {
        name: SSH_SERVICE_NAME.to_string(),
        port: ssh_server.port,
    }]);

    let discovery_server = VsockServer::create(move |stream| {
        let service_catalog = discovery_service.clone();
        async move {
            serve_discovery_connection(stream, service_catalog)
                .await
                .map_err(|err| io::Error::other(err.to_string()))
        }
    })
    .with_concurrency(64)
    .with_tracing(tracing::info_span!("vsock_server", service = "discovery"))
    .listen(Some(discovery_port))?;

    let _ = tokio::join!(ssh_server, discovery_server);
    Ok(())
}
