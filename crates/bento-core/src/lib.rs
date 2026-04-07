mod machine_id;
mod spec;

pub use crate::machine_id::MachineId;
pub use crate::spec::{
    Architecture, Backend, Boot, Bootstrap, Capabilities, Disk, DiskKind, Guest, GuestOs, Host,
    Mount, Network, NetworkMode, Platform, Resources, Storage, VmSpec,
};
