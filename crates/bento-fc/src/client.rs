use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api;
use crate::api::types::{
    BootSource, Drive, FirecrackerVersion, FullVmConfiguration, InstanceActionInfo,
    InstanceActionInfoActionType, InstanceInfo, MachineConfiguration, SerialDevice, Vm, VmState,
    Vsock,
};
use crate::connection::{api_client, DEFAULT_TIMEOUT};
use crate::error::FirecrackerError;

#[derive(Debug)]
pub struct FirecrackerClient {
    inner: api::Client,
    socket_path: PathBuf,
    timeout: Duration,
}

impl FirecrackerClient {
    pub fn connect(socket_path: impl AsRef<Path>) -> Result<Self, FirecrackerError> {
        Self::connect_with_timeout(socket_path, DEFAULT_TIMEOUT)
    }

    pub fn connect_with_timeout(
        socket_path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, FirecrackerError> {
        let socket_path = socket_path.as_ref().to_path_buf();
        let inner = api_client(&socket_path, timeout)?;
        Ok(Self {
            inner,
            socket_path,
            timeout,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn raw(&self) -> &api::Client {
        &self.inner
    }

    pub fn into_raw(self) -> api::Client {
        self.inner
    }

    pub async fn describe_instance(&self) -> Result<InstanceInfo, FirecrackerError> {
        Ok(self
            .inner
            .describe_instance()
            .send()
            .await
            .map_err(FirecrackerError::api)?
            .into_inner())
    }

    pub async fn configure_boot_source(
        &self,
        boot_source: BootSource,
    ) -> Result<(), FirecrackerError> {
        self.inner
            .put_guest_boot_source()
            .body(boot_source)
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }

    pub async fn configure_machine(
        &self,
        configuration: MachineConfiguration,
    ) -> Result<(), FirecrackerError> {
        self.inner
            .put_machine_configuration()
            .body(configuration)
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }

    pub async fn configure_drive(&self, drive: Drive) -> Result<(), FirecrackerError> {
        self.inner
            .put_guest_drive_by_id()
            .drive_id(&drive.drive_id)
            .body(drive)
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }

    pub async fn configure_vsock(&self, vsock: Vsock) -> Result<(), FirecrackerError> {
        self.inner
            .put_guest_vsock()
            .body(vsock)
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }

    pub async fn start_instance(&self) -> Result<(), FirecrackerError> {
        self.inner
            .create_sync_action()
            .body(api::types::InstanceActionInfo {
                action_type: InstanceActionInfoActionType::InstanceStart,
            })
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }

    pub async fn configure_serial(&self, serial: SerialDevice) -> Result<(), FirecrackerError> {
        self.inner
            .put_serial_device()
            .body(serial)
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }

    pub async fn version(&self) -> Result<FirecrackerVersion, FirecrackerError> {
        Ok(self
            .inner
            .get_firecracker_version()
            .send()
            .await
            .map_err(FirecrackerError::api)?
            .into_inner())
    }

    pub async fn config(&self) -> Result<FullVmConfiguration, FirecrackerError> {
        Ok(self
            .inner
            .get_export_vm_config()
            .send()
            .await
            .map_err(FirecrackerError::api)?
            .into_inner())
    }

    pub async fn machine_configuration(&self) -> Result<MachineConfiguration, FirecrackerError> {
        Ok(self
            .inner
            .get_machine_configuration()
            .send()
            .await
            .map_err(FirecrackerError::api)?
            .into_inner())
    }

    pub async fn set_vm_state(&self, state: VmState) -> Result<(), FirecrackerError> {
        self.inner
            .patch_vm()
            .body(Vm { state })
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }

    pub async fn send_action(
        &self,
        action_type: InstanceActionInfoActionType,
    ) -> Result<(), FirecrackerError> {
        self.inner
            .create_sync_action()
            .body(InstanceActionInfo { action_type })
            .send()
            .await
            .map_err(FirecrackerError::api)?;
        Ok(())
    }
}
