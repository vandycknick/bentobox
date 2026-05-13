//! Safe Rust abstractions over Apple's Virtualization.framework for BentoBox.

#[cfg(not(target_os = "macos"))]
compile_error!("bento-vz only supports macOS hosts");

mod error;
mod vm;
mod vz_ext;

pub mod configuration;
pub mod device;
pub mod dispatch;

mod utils;

pub use crate::configuration::{
    GenericMachineIdentifier, GenericPlatform, LinuxBootLoader, VirtualMachineConfiguration,
};
pub use crate::error::VzError;
pub use crate::utils::{rosetta_availability, RosettaAvailability};
pub use crate::vm::{VirtualMachine, VirtualMachineDelegate, VirtualMachineState};
