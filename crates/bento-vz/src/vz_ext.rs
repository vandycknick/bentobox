#![allow(non_snake_case)]

use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineConfiguration};

use crate::dispatch::ffi::dispatch_queue_t;

pub(crate) trait VZVirtualMachineExt {
    unsafe fn initWithConfiguration_queue(
        this: Allocated<Self>,
        configuration: &VZVirtualMachineConfiguration,
        queue: dispatch_queue_t,
    ) -> Retained<Self>
    where
        Self: Sized;
}

impl VZVirtualMachineExt for VZVirtualMachine {
    unsafe fn initWithConfiguration_queue(
        this: Allocated<Self>,
        configuration: &VZVirtualMachineConfiguration,
        queue: dispatch_queue_t,
    ) -> Retained<Self> {
        msg_send![this, initWithConfiguration: configuration, queue: queue]
    }
}
