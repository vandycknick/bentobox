pub mod agent;
mod machine_id;
mod spec;

pub use crate::machine_id::{looks_like_id_prefix, MachineId, MachineIdParseError, SHORT_ID_LEN};
pub use crate::spec::{
    Architecture, BackoffSpec, Boot, Bootstrap, Disk, DiskKind, GuestOs, GuestSpec, LifecycleSpec,
    Mount, Platform, PluginSpec, Resources, RestartPolicy, Settings, Storage, VmSpec,
    VsockEndpointMode, VsockEndpointSpec, DEFAULT_GUEST_CONTROL_PORT,
};
