mod config;
mod port;
mod port_forward;
mod server;
mod services;

use bento_protocol::guest::v1::ServiceEndpoint;
use bento_runtime::extensions::BuiltinExtension;
use bento_runtime::services::{SERVICE_DOCKER, SERVICE_SSH};
use std::io;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpStream, UnixStream};

use crate::config::GuestdConfig;
use crate::port_forward::PortForwardManager;
use crate::server::VsockServer;
use crate::services::{serve_discovery_connection, GuestDiscoveryState};

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

    let guestd_config = GuestdConfig::load()?;
    let discovery_port = crate::port::from_kernel_cmdline();
    let mut service_endpoints = Vec::new();
    let mut running_servers = Vec::new();
    let mut background_tasks = Vec::new();
    let port_forward_manager = if guestd_config
        .extensions
        .is_enabled(BuiltinExtension::PortForward)
    {
        let (manager, task) = PortForwardManager::spawn();
        background_tasks.push(task);
        Some(manager)
    } else {
        None
    };

    if guestd_config.extensions.is_enabled(BuiltinExtension::Ssh) {
        let ssh_server = VsockServer::create(|mut stream| async move {
            let mut ssh = TcpStream::connect("127.0.0.1:22").await?;
            let _ = copy_bidirectional(&mut stream, &mut ssh).await?;
            Ok(())
        })
        .with_concurrency(256)
        .with_tracing(tracing::info_span!("vsock_server", service = SERVICE_SSH))
        .listen(None)?;

        service_endpoints.push(ServiceEndpoint {
            name: SERVICE_SSH.to_string(),
            port: ssh_server.port,
        });
        running_servers.push(ssh_server);
    }

    if guestd_config
        .extensions
        .is_enabled(BuiltinExtension::Docker)
    {
        let docker_server = VsockServer::create(|mut stream| async move {
            let mut docker = UnixStream::connect("/var/run/docker.sock").await?;
            let _ = copy_bidirectional(&mut stream, &mut docker).await?;
            Ok(())
        })
        .with_concurrency(256)
        .with_tracing(tracing::info_span!(
            "vsock_server",
            service = SERVICE_DOCKER
        ))
        .listen(None)?;

        service_endpoints.push(ServiceEndpoint {
            name: SERVICE_DOCKER.to_string(),
            port: docker_server.port,
        });
        running_servers.push(docker_server);
    }

    tracing::info!(services = ?service_endpoints, "setting up services for discovery");

    let discovery_service = GuestDiscoveryState::new(
        service_endpoints,
        guestd_config.extensions,
        guestd_config.mounts,
        port_forward_manager,
    );

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

    running_servers.push(discovery_server);

    let mut join_set = tokio::task::JoinSet::new();
    for server in running_servers {
        join_set.spawn(server.wait());
    }
    for task in background_tasks {
        join_set.spawn(task);
    }

    while let Some(result) = join_set.join_next().await {
        result??;
    }

    Ok(())
}
