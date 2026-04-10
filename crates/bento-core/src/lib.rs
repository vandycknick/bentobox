pub mod capabilities;
mod instance_file;
mod machine_id;
mod mount_path;
pub mod services;
mod spec;

pub use crate::instance_file::InstanceFile;
pub use crate::machine_id::{looks_like_id_prefix, MachineId, MachineIdParseError, SHORT_ID_LEN};
pub use crate::mount_path::resolve_mount_location;
pub use crate::spec::{
    Architecture, Backend, Boot, Bootstrap, Disk, DiskKind, GuestOs, Mount, Network, NetworkMode,
    Platform, Resources, Settings, Storage, VmSpec,
};
