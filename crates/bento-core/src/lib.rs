pub mod capabilities;
mod instance_file;
mod machine_id;
mod mount_path;
mod spec;

pub use crate::instance_file::InstanceFile;
pub use crate::machine_id::{looks_like_id_prefix, MachineId, MachineIdParseError, SHORT_ID_LEN};
pub use crate::mount_path::resolve_mount_location;
pub use crate::spec::{
    Architecture, Backend, Boot, Bootstrap, Capabilities, Disk, DiskKind, Guest, GuestOs, Host,
    Mount, Network, NetworkMode, Platform, Resources, Storage, VmSpec,
};
