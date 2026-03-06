#![allow(
    dead_code,
    private_interfaces,
    clippy::missing_transmute_annotations,
    clippy::unused_unit,
    clippy::wrong_self_convention
)]

pub mod ffi;
mod queue;
mod suspend;

pub use queue::Queue;
pub use queue::QueueAttribute;
