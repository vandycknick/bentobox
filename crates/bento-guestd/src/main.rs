#![cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, unused_imports, unused_variables)
)]

#[cfg(not(target_os = "linux"))]
compile_error!("bento-guestd only supports Linux guests");

mod config;
mod dns;
mod host;
mod init;
mod port;
mod port_forward;
mod rpc;
mod server;

use std::io;

use bento_protocol::v1::{EndpointDescriptor, EndpointKind};
use bento_runtime::profiles::ENDPOINT_SSH;
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;

use crate::config::GuestdConfig;
use crate::dns::DnsServer;
use crate::port_forward::ForwardRuntime;
use crate::rpc::{serve_agent_connection, AgentContext};
use crate::server::VsockServer;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> eyre::Result<()> {
    let is_pid1 = std::process::id() == 1;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .with_writer(std::io::stderr)
        .try_init();

    if is_pid1 {
        tracing::info!("Running as PID 1, configuring system");
    }

    tracing::info!("agent starting");

    let guestd_config = GuestdConfig::load()?;
    let discovery_port = crate::port::from_kernel_cmdline();
    let mut endpoints = Vec::<EndpointDescriptor>::new();
    let mut running_servers = Vec::new();
    let dns_server = if guestd_config.capabilities.dns.enabled {
        let dns_server = DnsServer::new(&guestd_config.capabilities.dns).await?;
        DnsServer::write_resolv_conf(Some(guestd_config.capabilities.dns.listen_address))?;
        Some(dns_server)
    } else {
        None
    };

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
    let cancel = tokio_util::sync::CancellationToken::new();

    let dns_handle = dns_server.map(|dns_server| {
        let token = cancel.clone();
        tokio::spawn(async move {
            if let Err(err) = dns_server.run(token).await {
                tracing::error!(error = %err, "dns server exited unexpectedly");
            }
        })
    });

    while let Some(result) = join_set.join_next().await {
        result??;
    }

    // Shut down background tasks.
    cancel.cancel();
    if let Some(dns_handle) = dns_handle {
        let _ = tokio::join!(dns_handle);
    }

    Ok(())
}
