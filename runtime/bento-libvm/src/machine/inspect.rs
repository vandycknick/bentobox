use std::collections::BTreeMap;
use std::path::PathBuf;

use bento_vm_spec::VmSpec;

use crate::models::{MachineConfig, MachineRuntimeState, MachineState};
use crate::network::RequestedNetwork;

/// Public snapshot returned by machine inspect and mutation operations.
///
/// This is intentionally flattened so callers can read machine configuration
/// and persisted runtime state without depending on the crate's internal store
/// models.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MachineInspectData {
    /// Stable machine ID.
    pub id: String,
    /// Human-readable machine name.
    pub name: String,
    /// VM specification used to start the machine.
    pub spec: VmSpec,
    /// Directory containing this machine's persistent runtime files.
    pub instance_dir: PathBuf,
    /// Unix timestamp for when the machine was created.
    pub created_at: i64,
    /// Unix timestamp for the last configuration change.
    pub modified_at: i64,
    /// Image reference used to create the machine.
    pub image_ref: String,
    /// Requested root disk size in bytes, when explicitly configured.
    pub root_disk_size: Option<u64>,
    /// User-defined labels attached to the machine.
    pub labels: BTreeMap<String, String>,
    /// User-defined metadata attached to the machine.
    pub metadata: BTreeMap<String, String>,
    /// Requested network attachment.
    pub network: RequestedNetwork,
    /// Persisted machine lifecycle state.
    pub status: MachineStatus,
    /// Process ID for the running monitor, when known.
    pub vmmon_pid: Option<i32>,
    /// Unix timestamp for when the machine last started.
    pub started_at: Option<i64>,
    /// Last persisted runtime error, when present.
    pub last_error: Option<String>,
    /// Unix timestamp for the last runtime state change.
    pub updated_at: i64,
}

impl MachineInspectData {
    pub(crate) fn from_models(config: MachineConfig, state: MachineState) -> Self {
        Self {
            id: config.id.to_string(),
            name: config.name,
            spec: config.spec,
            instance_dir: config.instance_dir,
            created_at: config.created_at,
            modified_at: config.modified_at,
            image_ref: config.image_ref,
            root_disk_size: config.root_disk_size,
            labels: config.labels,
            metadata: config.metadata,
            network: config.network.into(),
            status: state.status.into(),
            vmmon_pid: state.vmmon_pid,
            started_at: state.started_at,
            last_error: state.last_error,
            updated_at: state.updated_at,
        }
    }

    /// Returns true when the persisted lifecycle state is running.
    pub fn is_running(&self) -> bool {
        self.status.is_running()
    }

    /// Returns the vmmon trace log path for this machine.
    pub fn trace_log_path(&self) -> PathBuf {
        crate::paths::vmmon_trace_log_path_in(&self.instance_dir)
    }
}

/// Persisted machine lifecycle state.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineStatus {
    /// The machine is stopped.
    Stopped,
    /// The machine is starting.
    Starting,
    /// The machine is running.
    Running,
    /// The machine is stopping.
    Stopping,
    /// The machine is in an error state.
    Error,
}

impl MachineStatus {
    /// Returns true when the lifecycle state is running.
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
}

impl From<MachineRuntimeState> for MachineStatus {
    fn from(value: MachineRuntimeState) -> Self {
        match value {
            MachineRuntimeState::Stopped => Self::Stopped,
            MachineRuntimeState::Starting => Self::Starting,
            MachineRuntimeState::Running => Self::Running,
            MachineRuntimeState::Stopping => Self::Stopping,
            MachineRuntimeState::Error => Self::Error,
        }
    }
}

impl From<MachineStatus> for MachineRuntimeState {
    fn from(value: MachineStatus) -> Self {
        match value {
            MachineStatus::Stopped => Self::Stopped,
            MachineStatus::Starting => Self::Starting,
            MachineStatus::Running => Self::Running,
            MachineStatus::Stopping => Self::Stopping,
            MachineStatus::Error => Self::Error,
        }
    }
}
