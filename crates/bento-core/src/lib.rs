pub mod capabilities;
pub mod forward;
mod instance_file;
mod machine_id;
mod mount_path;
pub mod services;
mod spec;

pub use crate::instance_file::InstanceFile;
pub use crate::machine_id::{looks_like_id_prefix, MachineId, MachineIdParseError, SHORT_ID_LEN};
pub use crate::mount_path::resolve_mount_location;
pub use crate::spec::{
    Architecture, Backend, BackoffSpec, Boot, Bootstrap, Disk, DiskKind, EndpointMode,
    EndpointSpec, GuestOs, LifecycleSpec, Mount, Network, NetworkMode, Platform, PluginSpec,
    Resources, RestartPolicy, Settings, Storage, VmSpec,
};
