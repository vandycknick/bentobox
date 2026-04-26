use std::path::PathBuf;

use crate::api::types::{VmInfo, VsockConfig};
use crate::client::CloudHypervisorClient;
use crate::error::CloudHypervisorError;
use crate::vsock::VsockDevice;

#[derive(Debug)]
pub struct VirtualMachine {
    client: CloudHypervisorClient,
    vsock: Option<ConfiguredVsock>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfiguredVsock {
    guest_cid: u32,
    socket_path: PathBuf,
}

impl VirtualMachine {
    pub(crate) fn new(client: CloudHypervisorClient, vsock: Option<ConfiguredVsock>) -> Self {
        Self { client, vsock }
    }

    pub fn client(&self) -> &CloudHypervisorClient {
        &self.client
    }

    pub fn into_client(self) -> CloudHypervisorClient {
        self.client
    }

    pub async fn info(&self) -> Result<VmInfo, CloudHypervisorError> {
        self.client.vm_info().await
    }

    pub async fn shutdown(&self) -> Result<(), CloudHypervisorError> {
        self.client.shutdown_vm().await
    }

    pub fn vsock(&self) -> Result<VsockDevice, CloudHypervisorError> {
        let vsock = self
            .vsock
            .as_ref()
            .ok_or(CloudHypervisorError::VsockNotConfigured)?;
        Ok(VsockDevice::new(vsock.guest_cid, vsock.socket_path.clone()))
    }
}

impl TryFrom<VsockConfig> for ConfiguredVsock {
    type Error = CloudHypervisorError;

    fn try_from(value: VsockConfig) -> Result<Self, Self::Error> {
        let guest_cid = u32::try_from(value.cid).map_err(|_| {
            CloudHypervisorError::InvalidVsockHandshake(format!(
                "guest CID must fit in u32, got {}",
                value.cid
            ))
        })?;

        Ok(Self {
            guest_cid,
            socket_path: PathBuf::from(value.socket),
        })
    }
}
