use crate::async_fd::AsyncFdStream;
use bento_protocol::{GuestDiscoveryClient, HealthStatus, DEFAULT_DISCOVERY_PORT};
use bento_runtime::driver::{Driver, OpenDeviceRequest, OpenDeviceResponse};
use bento_runtime::instance_control::{SERVICE_SERIAL, SERVICE_SSH};
use eyre::Context;
use std::collections::BTreeMap;
use std::time::Duration;
use tarpc::context;
use tarpc::serde_transport;
use tarpc::tokio_serde::formats::Bincode;
use tarpc::tokio_util::codec::LengthDelimitedCodec;

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
    pub(crate) async fn discover(driver: &dyn Driver) -> eyre::Result<Self> {
        let mut by_name = BTreeMap::new();
        by_name.insert(SERVICE_SERIAL.to_string(), ServiceTarget::Serial);

        let vsock_fd = match driver.open_device(OpenDeviceRequest::Vsock {
            port: DEFAULT_DISCOVERY_PORT,
        })? {
            OpenDeviceResponse::Vsock { stream } => stream,
            OpenDeviceResponse::Serial { .. } => {
                eyre::bail!("driver returned serial device when opening guest discovery port")
            }
        };

        let stream = AsyncFdStream::new(std::fs::File::from(vsock_fd))
            .context("wrap discovery stream in async fd")?;
        let framed = LengthDelimitedCodec::builder().new_framed(stream);
        let transport = serde_transport::new(framed, Bincode::default());
        let client = GuestDiscoveryClient::new(tarpc::client::Config::default(), transport).spawn();

        let HealthStatus { ok } =
            tokio::time::timeout(Duration::from_secs(3), client.health(context::current()))
                .await
                .map_err(|_| eyre::eyre!("guest discovery health request timed out"))?
                .map_err(|err| eyre::eyre!("query guest discovery health failed: {err}"))?;

        if !ok {
            eyre::bail!("guest discovery service reported unhealthy");
        }

        let endpoints = tokio::time::timeout(
            Duration::from_secs(3),
            client.list_services(context::current()),
        )
        .await
        .map_err(|_| eyre::eyre!("guest discovery list_services request timed out"))?
        .map_err(|err| eyre::eyre!("query guest service list failed: {err}"))?;

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
