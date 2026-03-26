mod backend;
mod machine;
mod serial;
mod stream;
mod types;

pub use crate::machine::{VirtualMachine, Vmm};
pub use crate::serial::{spawn_serial_tunnel, SerialAccess, SerialConsole, SerialStream};
pub use crate::stream::VsockStream;
pub use crate::types::{
    Backend, DiskImage, MachineIdentifier, NetworkMode, SharedDirectory, VmConfig, VmConfigBuilder,
    VmExit, VmmError,
};
