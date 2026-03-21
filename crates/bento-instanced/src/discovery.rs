use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bento_machine::MachineInstance;
use bento_protocol::guest::v1::guest_discovery_service_client::GuestDiscoveryServiceClient;
use bento_protocol::guest::v1::{
    ExtensionStatus, HealthRequest, ListExtensionsRequest, ListServicesRequest, ServiceStatus,
    ShutdownRequest,
};
use bento_protocol::DEFAULT_DISCOVERY_PORT;
use bento_runtime::services::SERVICE_SERIAL;
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
    extensions: BTreeMap<String, ExtensionStatus>,
}

impl ServiceRegistry {
    pub(crate) async fn discover(machine: &MachineInstance) -> eyre::Result<Self> {
        let mut by_name = BTreeMap::new();
        by_name.insert(SERVICE_SERIAL.to_string(), ServiceTarget::Serial);

        let mut client = connect_guest_client(machine).await?;
        verify_guest_health(&mut client).await?;

        let endpoints = tokio::time::timeout(
            Duration::from_secs(3),
            client.list_services(ListServicesRequest {}),
        )
        .await
        .map_err(|_| eyre::eyre!("guest discovery list_services request timed out"))?
        .map_err(|err| eyre::eyre!("query guest service list failed: {err}"))?
        .into_inner()
        .services;

        for endpoint in endpoints {
            by_name.insert(endpoint.name, ServiceTarget::VsockPort(endpoint.port));
        }

        let extensions = tokio::time::timeout(
            Duration::from_secs(3),
            client.list_extensions(ListExtensionsRequest {}),
        )
        .await
        .map_err(|_| eyre::eyre!("guest discovery list_extensions request timed out"))?
        .map_err(|err| eyre::eyre!("query guest extension list failed: {err}"))?
        .into_inner();

        let extensions = extensions
            .extensions
            .into_iter()
            .map(|extension| (extension.name.clone(), extension))
            .collect();

        Ok(Self {
            by_name,
            extensions,
        })
    }

    pub(crate) fn resolve(&self, name: &str) -> Option<ServiceTarget> {
        self.by_name.get(name).copied()
    }

    pub(crate) fn extensions(&self) -> impl Iterator<Item = &ExtensionStatus> {
        self.extensions.values()
    }
}

pub(crate) async fn request_guest_shutdown(
    machine: &MachineInstance,
    reboot: bool,
) -> eyre::Result<()> {
    let mut client = connect_guest_client(machine).await?;
    tokio::time::timeout(
        Duration::from_secs(3),
        client.shutdown(ShutdownRequest { reboot }),
    )
    .await
    .map_err(|_| eyre::eyre!("guest discovery shutdown request timed out"))?
    .map_err(|err| eyre::eyre!("guest discovery shutdown request failed: {err}"))?;
    Ok(())
}

async fn connect_guest_client(
    machine: &MachineInstance,
) -> eyre::Result<GuestDiscoveryServiceClient<tonic::transport::Channel>> {
    let stream = machine.open_vsock(DEFAULT_DISCOVERY_PORT).await?;
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

    Ok(GuestDiscoveryServiceClient::new(channel))
}

async fn verify_guest_health(
    client: &mut GuestDiscoveryServiceClient<tonic::transport::Channel>,
) -> eyre::Result<()> {
    let health = tokio::time::timeout(Duration::from_secs(3), client.health(HealthRequest {}))
        .await
        .map_err(|_| eyre::eyre!("guest discovery health request timed out"))?
        .map_err(|err| eyre::eyre!("query guest discovery health failed: {err}"))?
        .into_inner();

    if !health.ok {
        eyre::bail!("guest discovery service reported unhealthy");
    }

    if let Some(unhealthy) = health
        .services
        .iter()
        .find(|service| service.status != ServiceStatus::Running as i32)
    {
        eyre::bail!(
            "guest discovery service {} is unhealthy: {}",
            unhealthy.name,
            unhealthy.message
        );
    }

    Ok(())
}
