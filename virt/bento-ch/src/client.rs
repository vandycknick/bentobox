use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api;
use crate::api::types::{FsConfig, PciDeviceInfo, VmConfig, VmInfo, VmmPingResponse, VsockConfig};
use crate::connection::{api_client, DEFAULT_TIMEOUT};
use crate::error::CloudHypervisorError;

#[derive(Debug)]
pub struct CloudHypervisorClient {
    inner: api::Client,
    socket_path: PathBuf,
    timeout: Duration,
}

impl CloudHypervisorClient {
    pub fn connect(socket_path: impl AsRef<Path>) -> Result<Self, CloudHypervisorError> {
        Self::connect_with_timeout(socket_path, DEFAULT_TIMEOUT)
    }

    pub fn connect_with_timeout(
        socket_path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, CloudHypervisorError> {
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

    pub async fn ping_vmm(&self) -> Result<VmmPingResponse, CloudHypervisorError> {
        Ok(self
            .inner
            .get_vmm_ping()
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("get_vmm_ping", err))?
            .into_inner())
    }

    pub async fn create_vm(&self, config: VmConfig) -> Result<(), CloudHypervisorError> {
        self.inner
            .create_vm()
            .body(config)
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("create_vm", err))?;
        Ok(())
    }

    pub async fn boot_vm(&self) -> Result<(), CloudHypervisorError> {
        self.inner
            .boot_vm()
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("boot_vm", err))?;
        Ok(())
    }

    pub async fn add_fs(&self, config: FsConfig) -> Result<PciDeviceInfo, CloudHypervisorError> {
        Ok(self
            .inner
            .put_vm_add_fs()
            .body(config)
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("put_vm_add_fs", err))?
            .into_inner())
    }

    pub async fn add_vsock(
        &self,
        config: VsockConfig,
    ) -> Result<PciDeviceInfo, CloudHypervisorError> {
        Ok(self
            .inner
            .put_vm_add_vsock()
            .body(config)
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("put_vm_add_vsock", err))?
            .into_inner())
    }

    pub async fn vm_info(&self) -> Result<VmInfo, CloudHypervisorError> {
        Ok(self
            .inner
            .get_vm_info()
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("get_vm_info", err))?
            .into_inner())
    }

    pub async fn shutdown_vm(&self) -> Result<(), CloudHypervisorError> {
        self.inner
            .shutdown_vm()
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("shutdown_vm", err))?;
        Ok(())
    }

    pub async fn shutdown_vmm(&self) -> Result<(), CloudHypervisorError> {
        self.inner
            .shutdown_vmm()
            .send()
            .await
            .map_err(|err| CloudHypervisorError::api("shutdown_vmm", err))?;
        Ok(())
    }
}
