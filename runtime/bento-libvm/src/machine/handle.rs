use std::time::Duration;

use bento_vm_spec::VmSpec;

use crate::machine::{MachineInspect, MachineRuntimeStatus, MachineUpdate};
use crate::network::RequestedNetwork;
use crate::runtime::Runtime;
use crate::{LibVmError, MachineId};

#[derive(Debug, Clone)]
pub struct Machine {
    runtime: Runtime,
    id: MachineId,
}

impl Machine {
    pub(crate) fn new(runtime: Runtime, id: MachineId) -> Self {
        Self { runtime, id }
    }

    pub fn id(&self) -> String {
        self.id.to_string()
    }

    pub async fn inspect(&self) -> Result<MachineInspect, LibVmError> {
        self.runtime.machine_inspect(self.id).await
    }

    pub async fn start(&self) -> Result<MachineInspect, LibVmError> {
        self.runtime.start_machine(self.id).await
    }

    pub async fn stop(&self) -> Result<MachineInspect, LibVmError> {
        self.runtime.stop_machine(self.id).await
    }

    pub async fn remove(self) -> Result<(), LibVmError> {
        self.runtime.remove_machine(self.id).await
    }

    pub async fn replace_config(&self, spec: VmSpec) -> Result<MachineInspect, LibVmError> {
        self.runtime.replace_machine_config(self.id, spec).await
    }

    pub async fn set_network(
        &self,
        network: RequestedNetwork,
    ) -> Result<MachineInspect, LibVmError> {
        self.runtime.set_machine_network(self.id, network).await
    }

    pub async fn update(&self, update: MachineUpdate) -> Result<MachineInspect, LibVmError> {
        self.runtime.update_machine(self.id, update).await
    }

    pub async fn wait_for_guest_running(&self, timeout: Duration) -> Result<(), LibVmError> {
        self.runtime.wait_for_guest_running(self.id, timeout).await
    }

    pub async fn get_status(&self) -> Result<MachineRuntimeStatus, LibVmError> {
        self.runtime.get_status(self.id).await
    }

    pub async fn open_serial_stream(&self) -> Result<tokio::net::UnixStream, LibVmError> {
        self.runtime.open_serial_stream(self.id).await
    }

    pub async fn open_shell_stream(
        &self,
        wait_for_guest_readiness: bool,
    ) -> Result<tokio::net::UnixStream, LibVmError> {
        self.runtime
            .open_shell_stream(self.id, wait_for_guest_readiness)
            .await
    }
}
