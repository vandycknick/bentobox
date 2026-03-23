use std::path::PathBuf;

use crate::api::types::{
    FirecrackerVersion, FullVmConfiguration, InstanceActionInfoActionType, InstanceInfo,
    MachineConfiguration, VmState, Vsock,
};
use crate::client::FirecrackerClient;
use crate::error::FirecrackerError;
use crate::vsock::VsockDevice;

#[derive(Debug)]
pub struct VirtualMachine {
    client: FirecrackerClient,
    vsock: Option<ConfiguredVsock>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfiguredVsock {
    guest_cid: u32,
    uds_path: PathBuf,
}

impl VirtualMachine {
    pub(crate) fn new(client: FirecrackerClient, vsock: Option<ConfiguredVsock>) -> Self {
        Self { client, vsock }
    }

    pub fn client(&self) -> &FirecrackerClient {
        &self.client
    }

    pub fn into_client(self) -> FirecrackerClient {
        self.client
    }

    pub async fn describe_instance(&self) -> Result<InstanceInfo, FirecrackerError> {
        self.client.describe_instance().await
    }

    pub async fn version(&self) -> Result<FirecrackerVersion, FirecrackerError> {
        self.client.version().await
    }

    pub async fn config(&self) -> Result<FullVmConfiguration, FirecrackerError> {
        self.client.config().await
    }

    pub async fn machine_configuration(&self) -> Result<MachineConfiguration, FirecrackerError> {
        self.client.machine_configuration().await
    }

    pub async fn pause(&self) -> Result<(), FirecrackerError> {
        self.client.set_vm_state(VmState::Paused).await
    }

    pub async fn resume(&self) -> Result<(), FirecrackerError> {
        self.client.set_vm_state(VmState::Resumed).await
    }

    pub async fn flush_metrics(&self) -> Result<(), FirecrackerError> {
        self.client
            .send_action(InstanceActionInfoActionType::FlushMetrics)
            .await
    }

    pub async fn send_ctrl_alt_del(&self) -> Result<(), FirecrackerError> {
        self.client
            .send_action(InstanceActionInfoActionType::SendCtrlAltDel)
            .await
    }

    pub fn vsock(&self) -> Result<VsockDevice, FirecrackerError> {
        let vsock = self
            .vsock
            .as_ref()
            .ok_or(FirecrackerError::VsockNotConfigured)?;
        Ok(VsockDevice::new(vsock.guest_cid, vsock.uds_path.clone()))
    }
}

impl TryFrom<Vsock> for ConfiguredVsock {
    type Error = FirecrackerError;

    fn try_from(value: Vsock) -> Result<Self, Self::Error> {
        let guest_cid = u32::try_from(value.guest_cid).map_err(|_| {
            FirecrackerError::InvalidVsockHandshake(format!(
                "guest CID must fit in u32, got {}",
                value.guest_cid
            ))
        })?;

        Ok(Self {
            guest_cid,
            uds_path: PathBuf::from(value.uds_path),
        })
    }
}
