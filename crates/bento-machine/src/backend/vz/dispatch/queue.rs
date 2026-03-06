use std::ffi::CString;

use block2::Block;

use super::{
    ffi::{
        dispatch_async, dispatch_get_global_queue, dispatch_get_main_queue, dispatch_queue_attr_t,
        dispatch_queue_create, dispatch_queue_t, dispatch_release, dispatch_retain, dispatch_sync,
        qos_class_t, DISPATCH_QUEUE_CONCURRENT, DISPATCH_QUEUE_SERIAL,
    },
    suspend::SuspendGuard,
};

#[derive(Debug)]
pub struct Queue {
    pub ptr: dispatch_queue_t,
}

impl Queue {
    pub fn create(label: &str, attr: QueueAttribute) -> Self {
        let label = CString::new(label).expect("queue label should not contain NUL bytes");
        let queue = unsafe { dispatch_queue_create(label.as_ptr(), attr.as_raw()) };
        Queue { ptr: queue }
    }

    #[allow(dead_code)]
    pub fn main() -> Self {
        let queue = dispatch_get_main_queue();
        unsafe {
            dispatch_retain(queue);
        }
        Queue { ptr: queue }
    }

    #[allow(dead_code)]
    pub fn global() -> Self {
        Queue::global_with_qos(DispatchQoSClass::Default)
    }

    #[allow(dead_code)]
    pub fn global_with_qos(qos: DispatchQoSClass) -> Self {
        unsafe {
            let queue = dispatch_get_global_queue(qos.as_raw(), 0);
            dispatch_retain(queue);
            Queue { ptr: queue }
        }
    }

    pub fn exec_block_async(&self, block: &Block<dyn Fn() -> ()>) {
        unsafe {
            dispatch_async(self.ptr, block);
        }
    }

    #[allow(dead_code)]
    pub fn exec_block_sync(&self, block: &Block<dyn Fn() -> ()>) {
        unsafe {
            dispatch_sync(self.ptr, block);
        }
    }

    #[allow(dead_code)]
    pub fn suspend(&self) -> SuspendGuard {
        SuspendGuard::new(self)
    }
}

unsafe impl Sync for Queue {}
unsafe impl Send for Queue {}

impl Clone for Queue {
    fn clone(&self) -> Self {
        unsafe {
            dispatch_retain(self.ptr);
        }
        Queue { ptr: self.ptr }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        unsafe {
            dispatch_release(self.ptr);
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub enum QueueAttribute {
    Serial,
    #[allow(dead_code)]
    Concurrent,
}

impl QueueAttribute {
    fn as_raw(&self) -> dispatch_queue_attr_t {
        match *self {
            QueueAttribute::Serial => DISPATCH_QUEUE_SERIAL,
            QueueAttribute::Concurrent => DISPATCH_QUEUE_CONCURRENT,
        }
    }
}

pub enum DispatchQoSClass {
    UserInteractive,
    UserInitiated,
    Default,
    Utility,
    Background,
    Unspecified,
}

impl DispatchQoSClass {
    pub fn as_raw(self) -> qos_class_t {
        match self {
            Self::UserInteractive => qos_class_t::QOS_CLASS_USER_INTERACTIVE,
            Self::UserInitiated => qos_class_t::QOS_CLASS_USER_INITIATED,
            Self::Default => qos_class_t::QOS_CLASS_DEFAULT,
            Self::Utility => qos_class_t::QOS_CLASS_UTILITY,
            Self::Background => qos_class_t::QOS_CLASS_BACKGROUND,
            Self::Unspecified => qos_class_t::QOS_CLASS_UNSPECIFIED,
        }
    }
}
