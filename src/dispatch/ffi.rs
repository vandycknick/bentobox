use std::ffi::{c_char, c_long, c_ulong, c_void};

use block2::Block;
use objc2::{Encode, Encoding, RefEncode};

#[repr(C)]
#[allow(non_camel_case_types)]
pub struct dispatch_object_s {
    _private: [u8; 0],
}

unsafe impl Encode for dispatch_object_s {
    const ENCODING: Encoding = Encoding::Object;
}

unsafe impl RefEncode for dispatch_object_s {
    const ENCODING_REF: Encoding = Encoding::Object;
}

#[allow(non_camel_case_types)]
pub(super) type dispatch_function_t = extern "C" fn(*mut c_void);
#[allow(non_camel_case_types)]
pub(super) type dispatch_object_t = *mut dispatch_object_s;
#[allow(non_camel_case_types)]
pub type dispatch_queue_t = *mut dispatch_object_s;
#[allow(non_camel_case_types)]
pub(super) type dispatch_queue_attr_t = *const dispatch_object_s;
#[allow(non_camel_case_types)]
pub(super) type dispatch_semaphore_t = *mut dispatch_object_s;
#[allow(non_camel_case_types)]
pub(super) type dispatch_time_t = u64;

/// Ref: https://github.com/apple-oss-distributions/libpthread/blob/c032e0b076700a0a47db75528a282b8d3a06531a/include/sys/qos.h#L130
#[derive(Debug, Copy, Clone)]
#[repr(u32)]
#[allow(non_camel_case_types)]
pub(super) enum qos_class_t {
    QOS_CLASS_USER_INTERACTIVE = 0x21,
    QOS_CLASS_USER_INITIATED = 0x19,
    QOS_CLASS_DEFAULT = 0x15,
    QOS_CLASS_UTILITY = 0x11,
    QOS_CLASS_BACKGROUND = 0x09,
    QOS_CLASS_UNSPECIFIED = 0x00,
}

extern "C" {
    static _dispatch_main_q: dispatch_object_s;
    static _dispatch_queue_attr_concurrent: dispatch_object_s;

    /// Returns a system-defined global concurrent queue with the specified quality-of-service class.
    ///
    /// Ref: https://developer.apple.com/documentation/dispatch/1452927-dispatch_get_global_queue
    pub(super) fn dispatch_get_global_queue(
        identifier: qos_class_t,
        flags: c_ulong,
    ) -> dispatch_queue_t;

    pub(super) fn dispatch_main();

    pub(super) fn dispatch_queue_create(
        label: *const c_char,
        attr: dispatch_queue_attr_t,
    ) -> dispatch_queue_t;

    pub(super) fn dispatch_async_f(
        queue: dispatch_queue_t,
        context: *mut c_void,
        work: dispatch_function_t,
    );
    pub(super) fn dispatch_async(queue: dispatch_queue_t, block: &Block<dyn Fn() -> ()>);
    pub(super) fn dispatch_sync_f(
        queue: dispatch_queue_t,
        context: *mut c_void,
        work: dispatch_function_t,
    );
    pub(super) fn dispatch_sync(queue: dispatch_queue_t, block: &Block<dyn Fn() -> ()>);

    pub(super) fn dispatch_release(object: dispatch_object_t);
    pub(super) fn dispatch_resume(object: dispatch_object_t);
    pub(super) fn dispatch_retain(object: dispatch_object_t);
    pub(super) fn dispatch_suspend(object: dispatch_object_t);

    pub(super) fn dispatch_semaphore_create(value: c_long) -> dispatch_semaphore_t;
    pub(super) fn dispatch_semaphore_signal(dsema: dispatch_semaphore_t) -> c_long;
    pub(super) fn dispatch_semaphore_wait(
        dsema: dispatch_semaphore_t,
        timeout: dispatch_time_t,
    ) -> c_long;

    pub fn dispatch_time(when: dispatch_time_t, delta: i64) -> dispatch_time_t;

}

pub const DISPATCH_QUEUE_SERIAL: dispatch_queue_attr_t = 0 as dispatch_queue_attr_t;
pub static DISPATCH_QUEUE_CONCURRENT: &dispatch_object_s =
    unsafe { &_dispatch_queue_attr_concurrent };

pub const DISPATCH_TIME_NOW: dispatch_time_t = 0;
pub const DISPATCH_TIME_FOREVER: dispatch_time_t = !0;

pub fn dispatch_get_main_queue() -> dispatch_queue_t {
    unsafe { &_dispatch_main_q as *const _ as dispatch_queue_t }
}
