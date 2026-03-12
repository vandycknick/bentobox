mod backend;
mod machine;
mod registry;
mod stream;
mod types;

pub use crate::machine::{Machine, MachineHandle};
pub use crate::stream::{SerialStream, VsockStream};
pub use crate::types::{
    DiskImage, MachineConfig, MachineError, MachineExitEvent, MachineExitReceiver, MachineId,
    MachineKind, MachineSpec, MachineState, NetworkMode, SharedDirectory,
};
