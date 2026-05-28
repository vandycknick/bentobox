use bento_core::MachineId;

use crate::models::{
    Machine, NetworkAttachment, NetworkDefinition, NetworkInstance, RequestedNetwork,
};
use crate::LibVmError;

pub(crate) trait Database: Sized + Clone + Send + Sync + 'static {
    type Settings: std::fmt::Debug + Clone + Send + Sync + 'static;

    async fn new(settings: &Self::Settings) -> Result<Self, LibVmError>;

    async fn insert_machine(&self, machine: &Machine) -> Result<(), LibVmError>;
    async fn update_machine_network(
        &self,
        machine_id: MachineId,
        network: &RequestedNetwork,
    ) -> Result<(), LibVmError>;
    async fn get_machine_by_id(&self, id: MachineId) -> Result<Option<Machine>, LibVmError>;
    async fn get_machine_by_name(&self, name: &str) -> Result<Option<Machine>, LibVmError>;
    async fn get_machine_by_id_prefix(&self, prefix: &str) -> Result<Vec<Machine>, LibVmError>;
    async fn list_machines(&self) -> Result<Vec<Machine>, LibVmError>;
    async fn allocate_ephemeral_name(&self, prefix: &str) -> Result<String, LibVmError>;
    async fn remove_machine(&self, machine: &Machine) -> Result<(), LibVmError>;

    async fn get_network_attachment(
        &self,
        machine_id: MachineId,
    ) -> Result<Option<NetworkAttachment>, LibVmError>;
    async fn get_network_instance(
        &self,
        network_id: &str,
    ) -> Result<Option<NetworkInstance>, LibVmError>;
    async fn upsert_network_instance(&self, instance: &NetworkInstance) -> Result<(), LibVmError>;
    async fn upsert_network_attachment(
        &self,
        attachment: &NetworkAttachment,
    ) -> Result<(), LibVmError>;
    async fn remove_network_attachment(&self, machine_id: MachineId) -> Result<(), LibVmError>;
    async fn remove_network_instance(&self, network_id: &str) -> Result<(), LibVmError>;
    async fn get_network_instance_by_definition(
        &self,
        definition_name: &str,
    ) -> Result<Option<NetworkInstance>, LibVmError>;
    async fn count_network_attachments(&self, network_id: &str) -> Result<u32, LibVmError>;
    async fn upsert_network_definition(
        &self,
        definition: &NetworkDefinition,
    ) -> Result<(), LibVmError>;
    async fn list_network_definitions(&self) -> Result<Vec<NetworkDefinition>, LibVmError>;
    async fn get_network_definition(
        &self,
        name: &str,
    ) -> Result<Option<NetworkDefinition>, LibVmError>;
    async fn remove_network_definition(&self, name: &str) -> Result<(), LibVmError>;
}
