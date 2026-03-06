use crate::async_fd::AsyncFdStream;
use bento_machine::{MachineHandle, OpenDeviceRequest, OpenDeviceResponse};
use bento_protocol::guest::v1::guest_discovery_service_client::GuestDiscoveryServiceClient;
use bento_protocol::guest::v1::{HealthRequest, ListServicesRequest, ServiceStatus};
use bento_protocol::DEFAULT_DISCOVERY_PORT;
use bento_runtime::services::{SERVICE_SERIAL, SERVICE_SSH};
use eyre::Context;
use hyper_util::rt::TokioIo;
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
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
}

impl ServiceRegistry {
    pub(crate) async fn discover(machine: &MachineHandle) -> eyre::Result<Self> {
        let mut by_name = BTreeMap::new();
        by_name.insert(SERVICE_SERIAL.to_string(), ServiceTarget::Serial);

        let vsock_fd = match machine
            .open_device(OpenDeviceRequest::Vsock {
                port: DEFAULT_DISCOVERY_PORT,
            })
            .await?
        {
            OpenDeviceResponse::Vsock { stream } => stream,
            OpenDeviceResponse::Serial { .. } => {
                eyre::bail!("driver returned serial device when opening guest discovery port")
            }
        };

        let stream = AsyncFdStream::new(std::fs::File::from(vsock_fd))
            .context("wrap discovery stream in async fd")?;
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

        let mut client = GuestDiscoveryServiceClient::new(channel);

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

        let endpoints = tokio::time::timeout(
            Duration::from_secs(3),
            client.list_services(ListServicesRequest {}),
        )
        .await
        .map_err(|_| eyre::eyre!("guest discovery list_services request timed out"))?
        .map_err(|err| eyre::eyre!("query guest service list failed: {err}"))?
        .into_inner()
        .services;

        if endpoints
            .iter()
            .all(|endpoint| endpoint.name != SERVICE_SSH)
        {
            eyre::bail!("guest discovery did not report ssh service");
        }

        for endpoint in endpoints {
            by_name.insert(endpoint.name, ServiceTarget::VsockPort(endpoint.port));
        }

        Ok(Self { by_name })
    }

    pub(crate) fn resolve(&self, name: &str) -> Option<ServiceTarget> {
        self.by_name.get(name).copied()
    }
}
