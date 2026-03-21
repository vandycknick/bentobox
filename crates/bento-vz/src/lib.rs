#![cfg(target_os = "macos")]

//! Safe Rust abstractions over Apple's Virtualization.framework for BentoBox.

mod error;
mod vm;
mod vz_ext;

pub mod configuration;
pub mod device;
pub mod dispatch;

mod utils;

pub use crate::configuration::{GenericPlatform, LinuxBootLoader, VirtualMachineConfiguration};
pub use crate::error::VzError;
pub use crate::vm::{VirtualMachine, VirtualMachineState};
