use std::{
    ffi::{c_void, CString},
    mem,
};

use block2::Block;

use super::{
    ffi::{
        dispatch_async, dispatch_async_f, dispatch_function_t, dispatch_get_global_queue,
        dispatch_get_main_queue, dispatch_queue_attr_t, dispatch_queue_create, dispatch_queue_t,
        dispatch_release, dispatch_retain, dispatch_sync, dispatch_sync_f, qos_class_t,
        DISPATCH_QUEUE_CONCURRENT, DISPATCH_QUEUE_SERIAL,
    },
    suspend::SuspendGuard,
};

/// A Grand Central Dispatch queue.
///
/// For more information, see Apple's [Grand Central Dispatch reference](
/// https://developer.apple.com/library/mac/documentation/Performance/Reference/GCD_libdispatch_Ref/index.html).
#[derive(Debug)]
pub struct Queue {
    pub ptr: dispatch_queue_t,
}

impl Queue {
    /// Creates a new dispatch `Queue`.
    pub fn create(label: &str, attr: QueueAttribute) -> Self {
        let label = CString::new(label).unwrap();
        let queue = unsafe { dispatch_queue_create(label.as_ptr(), attr.as_raw()) };
        Queue { ptr: queue }
    }

    pub fn main() -> Self {
        let queue = dispatch_get_main_queue();
        unsafe {
            dispatch_retain(queue);
        }
        Queue { ptr: queue }
    }

    pub fn global() -> Self {
        Queue::global_with_qos(DispatchQoSClass::Default)
    }

    pub fn global_with_qos(qos: DispatchQoSClass) -> Self {
        unsafe {
            let queue = dispatch_get_global_queue(qos.as_raw(), 0);
            dispatch_retain(queue);
            Queue { ptr: queue }
        }
    }

    /// Submits a closure for execution on self and waits until it completes.
    #[allow(dead_code)]
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

    /// Submits a closure for asynchronous execution on self and returns
    /// immediately.
    #[allow(dead_code)]
    pub fn exec_async<F>(&self, work: F)
    where
        F: 'static + Send + FnOnce(),
    {
        let (context, work) = context_and_function(work);
        unsafe {
            dispatch_async_f(self.ptr, context, work);
        }
    }

    #[allow(dead_code)]
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

    /// Suspends the invocation of blocks on self and returns a `SuspendGuard`
    /// that can be dropped to resume.
    ///
    /// The suspension occurs after completion of any blocks running at the
    /// time of the call.
    /// Invocation does not resume until all `SuspendGuard`s have been dropped.
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

/// The type of a dispatch queue.
#[derive(Clone, Debug, Hash, PartialEq)]
pub enum QueueAttribute {
    /// The queue executes blocks serially in FIFO order.
    Serial,
    /// The queue executes blocks concurrently.
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

/// Use quality-of-service classes to communicate the intent behind the work that your app performs.
/// The system uses those intentions to determine the best way to execute your tasks given the available resources.
/// For example, the system gives higher priority to threads that contain user-interactive tasks to ensure that those
/// tasks are executed quickly. Conversely, it gives lower priority to background tasks, and may attempt to save power
/// by executing them on more power-efficient CPU cores. The system determines how to execute your tasks dynamically
/// based on system conditions and the tasks you schedule.
///
/// Ref: https://developer.apple.com/documentation/dispatch/dispatchqos/qosclass
pub enum DispatchQoSClass {
    /// The quality-of-service class for user-interactive tasks, such as animations, event handling, or updating your app's user interface.
    UserInteractive,
    /// The quality-of-service class for tasks that prevent the user from actively using your app.
    UserInitiated,
    /// The default quality-of-service class.
    Default,
    /// The quality-of-service class for tasks that the user does not track actively.
    Utility,
    /// The quality-of-service class for maintenance or cleanup tasks that you create.
    Background,
    /// The absence of a quality-of-service class.
    Unspecified,
}

impl DispatchQoSClass {
    pub fn as_raw(self) -> qos_class_t {
        match self {
            Self::UserInteractive => qos_class_t::QOS_CLASS_USER_INTERACTIVE,
            Self::UserInitiated => qos_class_t::QOS_CLASS_USER_INITIATED,
            Self::Default => qos_class_t::QOS_CLASS_DEFAULT,
            Self::Utility => qos_class_t::QOS_CLASS_USER_INITIATED,
            Self::Background => qos_class_t::QOS_CLASS_BACKGROUND,
            Self::Unspecified => qos_class_t::QOS_CLASS_UNSPECIFIED,
        }
    }
}

fn context_and_function<F>(closure: F) -> (*mut c_void, dispatch_function_t)
where
    F: FnOnce(),
{
    extern "C" fn work_execute_closure<F>(context: Box<F>)
    where
        F: FnOnce(),
    {
        (*context)();
    }

    let closure = Box::new(closure);
    let func: extern "C" fn(Box<F>) = work_execute_closure::<F>;
    unsafe { (mem::transmute(closure), mem::transmute(func)) }
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
