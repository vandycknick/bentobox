mod machine_id;
mod spec;

pub use crate::machine_id::{looks_like_id_prefix, MachineId, MachineIdParseError, SHORT_ID_LEN};
pub use crate::spec::{
    Architecture, Backend, Boot, Bootstrap, Capabilities, Disk, DiskKind, Guest, GuestOs, Host,
    Mount, Network, NetworkMode, Platform, Resources, Storage, VmSpec,
};
