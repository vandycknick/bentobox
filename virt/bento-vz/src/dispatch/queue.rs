use std::{
    ffi::{c_void, CString},
    mem,
};

use block2::Block;

use crate::dispatch::ffi::{dispatch_function_t, dispatch_sync_f};

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
    pub(crate) ptr: dispatch_queue_t,
}

impl Queue {
    pub fn create(label: &str, attr: QueueAttribute) -> Self {
        let label = CString::new(label).expect("queue label should not contain NUL bytes");
        let queue = unsafe { dispatch_queue_create(label.as_ptr(), attr.as_raw()) };
        Self { ptr: queue }
    }

    pub fn exec_sync<T, F>(&self, work: F) -> T
    where
        F: Send + FnOnce() -> T,
        T: Send,
    {
        let mut result = None;
        {
            let result_ref = &mut result;
            let work = move || {
                *result_ref = Some(work());
            };

            let mut work = Some(work);
            let (context, work) = context_and_sync_function(&mut work);
            unsafe {
                dispatch_sync_f(self.ptr, context, work);
            }
        }
        // This was set so it's safe to unwrap
        result.unwrap()
    }

    pub fn exec_block_async(&self, block: &Block<dyn Fn() -> ()>) {
        unsafe { dispatch_async(self.ptr, block) }
    }

    pub fn exec_block_sync(&self, block: &Block<dyn Fn() -> ()>) {
        unsafe { dispatch_sync(self.ptr, block) }
    }

    #[allow(dead_code)]
    pub fn main() -> Self {
        let queue = dispatch_get_main_queue();
        unsafe { dispatch_retain(queue) };
        Self { ptr: queue }
    }

    #[allow(dead_code)]
    pub fn global_with_qos(qos: DispatchQoSClass) -> Self {
        unsafe {
            let queue = dispatch_get_global_queue(qos.as_raw(), 0);
            dispatch_retain(queue);
            Self { ptr: queue }
        }
    }

    #[allow(dead_code)]
    pub fn suspend(&self) -> SuspendGuard {
        SuspendGuard::new(self)
    }
}

unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

impl Clone for Queue {
    fn clone(&self) -> Self {
        unsafe { dispatch_retain(self.ptr) };
        Self { ptr: self.ptr }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        unsafe { dispatch_release(self.ptr) };
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
        match self {
            Self::Serial => DISPATCH_QUEUE_SERIAL,
            Self::Concurrent => DISPATCH_QUEUE_CONCURRENT,
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

fn context_and_sync_function<F>(closure: &mut Option<F>) -> (*mut c_void, dispatch_function_t)
where
    F: FnOnce(),
{
    extern "C" fn work_read_closure<F>(context: &mut Option<F>)
    where
        F: FnOnce(),
    {
        // This is always passed Some, so it's safe to unwrap
        let closure = context.take().unwrap();
        closure();
    }

    let context: *mut Option<F> = closure;
    let func: extern "C" fn(&mut Option<F>) = work_read_closure::<F>;
    unsafe { (context as *mut c_void, mem::transmute(func)) }
}
