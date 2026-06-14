use std::path::Path;
use std::time::Duration;

use bento_vm_spec::VmSpec;

use crate::machine::{
    Machine, MachineCreate, MachineInspectData, MachineRef, MachineRuntimeStatus, MachineUpdate,
};
use crate::models::MachineConfig;
use crate::network::{NetworkDefinition, RequestedNetwork};
use crate::paths::LocalPaths;
use crate::{LibVmError, MachineId};

use super::{
    local::LocalRuntime, remote::RemoteRuntime, RuntimeBackend, RuntimeConfig, RuntimeTarget,
};

/// Service-layer entry point for managing Bento virtual machines.
///
/// Use `Runtime` to create or resolve machines, then operate on them through
/// returned `Machine` handles.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub(crate) backend: RuntimeBackend,
}

impl Runtime {
    /// Opens a runtime from explicit configuration.
    pub async fn new(config: RuntimeConfig) -> Result<Self, LibVmError> {
        match config.target {
            RuntimeTarget::Local(config) => Ok(Self {
                backend: RuntimeBackend::Local(Box::new(
                    LocalRuntime::new(LocalPaths::new(config.data_dir), config.networking).await?,
                )),
            }),
            RuntimeTarget::Remote(config) => Ok(Self {
                backend: RuntimeBackend::Remote(RemoteRuntime::new(config)),
            }),
        }
    }

    /// Opens the default local runtime from the process environment.
    pub async fn from_env() -> Result<Self, LibVmError> {
        Self::new(RuntimeConfig::from_env()?).await
    }

    /// Returns the local data directory when this runtime uses the local backend.
    pub fn local_data_dir(&self) -> Option<&Path> {
        match &self.backend {
            RuntimeBackend::Local(local) => Some(local.paths().data_dir()),
            RuntimeBackend::Remote(_) => None,
        }
    }

    /// Returns the local image directory when this runtime uses the local backend.
    pub fn local_images_dir(&self) -> Option<&Path> {
        match &self.backend {
            RuntimeBackend::Local(local) => Some(local.paths().images_dir()),
            RuntimeBackend::Remote(_) => None,
        }
    }

    /// Creates a machine and returns an operable handle for it.
    pub async fn create_machine(&self, request: MachineCreate) -> Result<Machine, LibVmError> {
        let config = match &self.backend {
            RuntimeBackend::Local(local) => local.create_machine(request).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("create_machine"),
        }?;
        Ok(Machine::new(self.clone(), config.id))
    }

    /// Resolves a machine by name, full ID, or ID prefix.
    pub async fn get_machine(&self, machine: &MachineRef) -> Result<Machine, LibVmError> {
        let config = self.resolve_machine_config(machine).await?;
        Ok(Machine::new(self.clone(), config.id))
    }

    /// Lists known machines as operable handles.
    pub async fn list_machines(&self) -> Result<Vec<Machine>, LibVmError> {
        let configs = match &self.backend {
            RuntimeBackend::Local(local) => local.list_machine_configs().await,
            RuntimeBackend::Remote(remote) => remote.unsupported("list_machines"),
        }?;
        Ok(configs
            .into_iter()
            .map(|config| Machine::new(self.clone(), config.id))
            .collect())
    }

    /// Allocates an unused generated machine name using the provided prefix.
    pub async fn allocate_ephemeral_name(&self, prefix: &str) -> Result<String, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.allocate_ephemeral_name(prefix).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("allocate_ephemeral_name"),
        }
    }

    /// Creates a named network definition.
    pub async fn create_network_definition(
        &self,
        definition: NetworkDefinition,
    ) -> Result<(), LibVmError> {
        definition
            .validate()
            .map_err(|reason| LibVmError::InvalidCreateRequest {
                name: definition.name.clone(),
                reason,
            })?;
        let definition = definition.into();
        match &self.backend {
            RuntimeBackend::Local(local) => local.create_network_definition(definition).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("create_network_definition"),
        }
    }

    /// Lists all named network definitions.
    pub async fn list_network_definitions(&self) -> Result<Vec<NetworkDefinition>, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => Ok(local
                .list_network_definitions()
                .await?
                .into_iter()
                .map(Into::into)
                .collect()),
            RuntimeBackend::Remote(remote) => remote.unsupported("list_network_definitions"),
        }
    }

    /// Returns a named network definition when it exists.
    pub async fn get_network_definition(
        &self,
        name: &str,
    ) -> Result<Option<NetworkDefinition>, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => {
                Ok(local.get_network_definition(name).await?.map(Into::into))
            }
            RuntimeBackend::Remote(remote) => remote.unsupported("get_network_definition"),
        }
    }

    /// Removes a named network definition.
    pub async fn remove_network_definition(&self, name: &str) -> Result<(), LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.remove_network_definition(name).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("remove_network_definition"),
        }
    }

    pub(crate) async fn resolve_machine_config(
        &self,
        machine: &MachineRef,
    ) -> Result<MachineConfig, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.resolve_machine_config(machine).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("resolve_machine_config"),
        }
    }

    pub(crate) async fn inspect_machine(
        &self,
        id: MachineId,
    ) -> Result<MachineInspectData, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.inspect_by_id(id).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("inspect_machine"),
        }
    }

    pub(crate) async fn start_machine(
        &self,
        id: MachineId,
    ) -> Result<MachineInspectData, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.start_by_id(id).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("start_machine"),
        }
    }

    pub(crate) async fn stop_machine(
        &self,
        id: MachineId,
    ) -> Result<MachineInspectData, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.stop_by_id(id).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("stop_machine"),
        }
    }

    pub(crate) async fn remove_machine(&self, id: MachineId) -> Result<(), LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.remove_by_id(id).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("remove_machine"),
        }
    }

    pub(crate) async fn replace_machine_config(
        &self,
        id: MachineId,
        spec: VmSpec,
    ) -> Result<MachineInspectData, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.replace_config_by_id(id, spec).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("replace_machine_config"),
        }
    }

    pub(crate) async fn set_machine_network(
        &self,
        id: MachineId,
        network: RequestedNetwork,
    ) -> Result<MachineInspectData, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.set_network_by_id(id, network).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("set_machine_network"),
        }
    }

    pub(crate) async fn update_machine(
        &self,
        id: MachineId,
        update: MachineUpdate,
    ) -> Result<MachineInspectData, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.update_by_id(id, update).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("update_machine"),
        }
    }

    pub(crate) async fn wait_for_guest_running(
        &self,
        id: MachineId,
        timeout: Duration,
    ) -> Result<(), LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.wait_for_guest_running_by_id(id, timeout).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("wait_for_guest_running"),
        }
    }

    pub(crate) async fn get_status(
        &self,
        id: MachineId,
    ) -> Result<MachineRuntimeStatus, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.get_status_by_id(id).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("get_status"),
        }
    }

    pub(crate) async fn open_serial_stream(
        &self,
        id: MachineId,
    ) -> Result<tokio::net::UnixStream, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.open_serial_stream_by_id(id).await,
            RuntimeBackend::Remote(remote) => remote.unsupported("open_serial_stream"),
        }
    }

    pub(crate) async fn open_shell_stream(
        &self,
        id: MachineId,
        wait_for_guest_readiness: bool,
    ) -> Result<tokio::net::UnixStream, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => {
                local
                    .open_shell_stream_by_id(id, wait_for_guest_readiness)
                    .await
            }
            RuntimeBackend::Remote(remote) => remote.unsupported("open_shell_stream"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{LibVmError, Runtime, RuntimeConfig};

    #[tokio::test]
    async fn remote_runtime_has_no_local_paths() {
        let runtime = Runtime::new(RuntimeConfig::remote("http://127.0.0.1:8080"))
            .await
            .expect("create remote runtime stub");

        assert!(runtime.local_data_dir().is_none());
        assert!(runtime.local_images_dir().is_none());
    }

    #[tokio::test]
    async fn remote_runtime_reports_explicit_unsupported_operations() {
        let runtime = Runtime::new(RuntimeConfig::remote("http://127.0.0.1:8080"))
            .await
            .expect("create remote runtime stub");

        let err = runtime
            .allocate_ephemeral_name("bento")
            .await
            .expect_err("remote runtime should not implement operations yet");

        assert!(matches!(
            err,
            LibVmError::RemoteRuntimeUnsupported {
                endpoint,
                operation: "allocate_ephemeral_name"
            } if endpoint == "http://127.0.0.1:8080"
        ));
    }
}
