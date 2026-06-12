use std::path::Path;
use std::time::Duration;

use bento_vm_spec::VmSpec;

use crate::engine::LocalRuntime;
use crate::machine::{Machine, MachineCreate, MachineInspect, MachineRef, MachineRuntimeStatus};
use crate::models::MachineConfig;
use crate::network::{NetworkDefinition, RequestedNetwork};
use crate::paths::LocalPaths;
use crate::{LibVmError, MachineId};

use super::{RuntimeBackend, RuntimeConfig, RuntimeTarget};

#[derive(Debug, Clone)]
pub struct Runtime {
    pub(crate) backend: RuntimeBackend,
}

impl Runtime {
    pub async fn new(config: RuntimeConfig) -> Result<Self, LibVmError> {
        match config.target {
            RuntimeTarget::Local(config) => Ok(Self {
                backend: RuntimeBackend::Local(
                    LocalRuntime::new(LocalPaths::new(config.data_dir), config.networking).await?,
                ),
            }),
        }
    }

    pub async fn from_env() -> Result<Self, LibVmError> {
        Self::new(RuntimeConfig::from_env()?).await
    }

    pub fn local_data_dir(&self) -> Option<&Path> {
        match &self.backend {
            RuntimeBackend::Local(local) => Some(local.paths().data_dir()),
        }
    }

    pub fn local_images_dir(&self) -> Option<&Path> {
        match &self.backend {
            RuntimeBackend::Local(local) => Some(local.paths().images_dir()),
        }
    }

    pub async fn create_machine(&self, request: MachineCreate) -> Result<Machine, LibVmError> {
        let config = match &self.backend {
            RuntimeBackend::Local(local) => local.create_machine(request).await?,
        };
        Ok(Machine::new(self.clone(), config.id))
    }

    pub async fn get_machine(&self, machine: &MachineRef) -> Result<Machine, LibVmError> {
        let config = self.resolve_machine_config(machine).await?;
        Ok(Machine::new(self.clone(), config.id))
    }

    pub async fn list_machines(&self) -> Result<Vec<Machine>, LibVmError> {
        let configs = match &self.backend {
            RuntimeBackend::Local(local) => local.list_machine_configs().await?,
        };
        Ok(configs
            .into_iter()
            .map(|config| Machine::new(self.clone(), config.id))
            .collect())
    }

    pub async fn allocate_ephemeral_name(&self, prefix: &str) -> Result<String, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.allocate_ephemeral_name(prefix).await,
        }
    }

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
        let definition = definition.into_model();
        match &self.backend {
            RuntimeBackend::Local(local) => local.create_network_definition(definition).await,
        }
    }

    pub async fn list_network_definitions(&self) -> Result<Vec<NetworkDefinition>, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => Ok(local
                .list_network_definitions()
                .await?
                .into_iter()
                .map(NetworkDefinition::from_model)
                .collect()),
        }
    }

    pub async fn get_network_definition(
        &self,
        name: &str,
    ) -> Result<Option<NetworkDefinition>, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => Ok(local
                .get_network_definition(name)
                .await?
                .map(NetworkDefinition::from_model)),
        }
    }

    pub async fn remove_network_definition(&self, name: &str) -> Result<(), LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.remove_network_definition(name).await,
        }
    }

    pub(crate) async fn resolve_machine_config(
        &self,
        machine: &MachineRef,
    ) -> Result<MachineConfig, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.resolve_machine_config(machine).await,
        }
    }

    pub(crate) async fn machine_inspect(
        &self,
        id: MachineId,
    ) -> Result<MachineInspect, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.inspect_by_id(id).await,
        }
    }

    pub(crate) async fn start_machine(&self, id: MachineId) -> Result<MachineInspect, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.start_by_id(id).await,
        }
    }

    pub(crate) async fn stop_machine(&self, id: MachineId) -> Result<MachineInspect, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.stop_by_id(id).await,
        }
    }

    pub(crate) async fn remove_machine(&self, id: MachineId) -> Result<(), LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.remove_by_id(id).await,
        }
    }

    pub(crate) async fn replace_machine_config(
        &self,
        id: MachineId,
        spec: VmSpec,
    ) -> Result<MachineInspect, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.replace_config_by_id(id, spec).await,
        }
    }

    pub(crate) async fn set_machine_network(
        &self,
        id: MachineId,
        network: RequestedNetwork,
    ) -> Result<MachineInspect, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.set_network_by_id(id, network).await,
        }
    }

    pub(crate) async fn wait_for_guest_running(
        &self,
        id: MachineId,
        timeout: Duration,
    ) -> Result<(), LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.wait_for_guest_running_by_id(id, timeout).await,
        }
    }

    pub(crate) async fn get_status(
        &self,
        id: MachineId,
    ) -> Result<MachineRuntimeStatus, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.get_status_by_id(id).await,
        }
    }

    pub(crate) async fn open_serial_stream(
        &self,
        id: MachineId,
    ) -> Result<tokio::net::UnixStream, LibVmError> {
        match &self.backend {
            RuntimeBackend::Local(local) => local.open_serial_stream_by_id(id).await,
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
        }
    }
}
