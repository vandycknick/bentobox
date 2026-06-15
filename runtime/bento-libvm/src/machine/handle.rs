use std::time::Duration;

use bento_vm_spec::VmSpec;

use crate::machine::{
    MachineInspectData, MachineRuntimeStatus, MachineStartOptions, MachineUpdate,
};
use crate::network::RequestedNetwork;
use crate::runtime::Runtime;
use crate::{LibVmError, MachineId};

/// Handle for an operable Bento virtual machine.
///
/// A handle stores machine identity and routes operations through the `Runtime`
/// that created it. Use `inspect` to read the machine's current public snapshot.
#[derive(Debug, Clone)]
pub struct Machine {
    runtime: Runtime,
    id: MachineId,
}

impl Machine {
    pub(crate) fn new(runtime: Runtime, id: MachineId) -> Self {
        Self { runtime, id }
    }

    /// Returns the stable machine ID.
    pub fn id(&self) -> String {
        self.id.to_string()
    }

    /// Returns the latest persisted config and runtime state snapshot.
    pub async fn inspect(&self) -> Result<MachineInspectData, LibVmError> {
        self.runtime.inspect_machine(self.id).await
    }

    /// Starts the machine and returns its updated inspect data.
    pub async fn start(&self) -> Result<MachineInspectData, LibVmError> {
        self.start_with(MachineStartOptions::default()).await
    }

    /// Starts the machine with explicit start options.
    pub async fn start_with(
        &self,
        options: MachineStartOptions,
    ) -> Result<MachineInspectData, LibVmError> {
        self.runtime.start_machine(self.id, options).await
    }

    /// Stops the machine and returns its updated inspect data.
    pub async fn stop(&self) -> Result<MachineInspectData, LibVmError> {
        self.runtime.stop_machine(self.id).await
    }

    /// Cleans host resources associated with a stopped machine.
    ///
    /// This is safe to call repeatedly. If the machine is still active, cleanup
    /// leaves it alone and returns the current inspect data.
    pub async fn cleanup(&self) -> Result<MachineInspectData, LibVmError> {
        self.runtime.cleanup_machine(self.id).await
    }

    /// Removes the persistent machine record and files.
    pub async fn remove(self) -> Result<(), LibVmError> {
        self.runtime.remove_machine(self.id).await
    }

    /// Replaces the VM spec for a stopped machine.
    pub async fn replace_config(&self, spec: VmSpec) -> Result<MachineInspectData, LibVmError> {
        self.runtime.replace_machine_config(self.id, spec).await
    }

    /// Changes the requested network for a stopped machine.
    pub async fn set_network(
        &self,
        network: RequestedNetwork,
    ) -> Result<MachineInspectData, LibVmError> {
        self.runtime.set_machine_network(self.id, network).await
    }

    /// Applies partial settings updates to a stopped machine.
    pub async fn update(&self, update: MachineUpdate) -> Result<MachineInspectData, LibVmError> {
        self.runtime.update_machine(self.id, update).await
    }

    /// Waits until the guest agent reports the machine as running.
    pub async fn wait_for_guest_running(&self, timeout: Duration) -> Result<(), LibVmError> {
        self.runtime.wait_for_guest_running(self.id, timeout).await
    }

    /// Reads live monitor and guest component status.
    pub async fn get_status(&self) -> Result<MachineRuntimeStatus, LibVmError> {
        self.runtime.get_status(self.id).await
    }

    /// Opens the machine serial stream.
    pub async fn open_serial_stream(&self) -> Result<tokio::net::UnixStream, LibVmError> {
        self.runtime.open_serial_stream(self.id).await
    }

    /// Opens the guest shell stream.
    ///
    /// When `wait_for_guest_readiness` is true, this waits for the guest agent
    /// before opening the stream.
    pub async fn open_shell_stream(
        &self,
        wait_for_guest_readiness: bool,
    ) -> Result<tokio::net::UnixStream, LibVmError> {
        self.runtime
            .open_shell_stream(self.id, wait_for_guest_readiness)
            .await
    }
}
