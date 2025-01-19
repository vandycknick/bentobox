use std::error::Error;
use std::ffi::c_ulong;
use std::ffi::CString;
use std::fmt;
use std::mem;
use std::os::raw::{c_char, c_void};
use std::str;
use std::time::Duration;

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
pub type dispatch_function_t = extern "C" fn(*mut c_void);
#[allow(non_camel_case_types)]
pub type dispatch_object_t = *mut dispatch_object_s;
#[allow(non_camel_case_types)]
pub type dispatch_queue_t = *mut dispatch_object_s;
#[allow(non_camel_case_types)]
pub type dispatch_queue_attr_t = *const dispatch_object_s;
#[allow(non_camel_case_types)]
pub type dispatch_semaphore_t = *mut dispatch_object_s;
#[allow(non_camel_case_types)]
pub type dispatch_time_t = u64;

extern "C" {
    static _dispatch_main_q: dispatch_object_s;
    static _dispatch_queue_attr_concurrent: dispatch_object_s;

    /// Returns a system-defined global concurrent queue with the specified quality-of-service class.
    ///
    /// Ref: https://developer.apple.com/documentation/dispatch/1452927-dispatch_get_global_queue
    pub fn dispatch_get_global_queue(identifier: qos_class_t, flags: c_ulong) -> dispatch_queue_t;

    pub fn dispatch_main();

    pub fn dispatch_queue_create(
        label: *const c_char,
        attr: dispatch_queue_attr_t,
    ) -> dispatch_queue_t;

    pub fn dispatch_async_f(
        queue: dispatch_queue_t,
        context: *mut c_void,
        work: dispatch_function_t,
    );
    pub fn dispatch_async(queue: dispatch_queue_t, block: &Block<dyn Fn() -> ()>);
    pub fn dispatch_sync_f(
        queue: dispatch_queue_t,
        context: *mut c_void,
        work: dispatch_function_t,
    );
    pub fn dispatch_sync(queue: dispatch_queue_t, block: &Block<dyn Fn() -> ()>);

    pub fn dispatch_release(object: dispatch_object_t);
    pub fn dispatch_resume(object: dispatch_object_t);
    pub fn dispatch_retain(object: dispatch_object_t);
    pub fn dispatch_suspend(object: dispatch_object_t);

    pub fn dispatch_semaphore_create(value: c_long) -> dispatch_semaphore_t;
    pub fn dispatch_semaphore_signal(dsema: dispatch_semaphore_t) -> c_long;
    pub fn dispatch_semaphore_wait(dsema: dispatch_semaphore_t, timeout: dispatch_time_t)
        -> c_long;

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

/// An error indicating a wait timed out.
#[derive(Clone, Debug)]
pub struct WaitTimeout {
    duration: Duration,
}

impl fmt::Display for WaitTimeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Wait timed out after duration {:?}", self.duration)
    }
}

impl Error for WaitTimeout {}

fn time_after_delay(delay: Duration) -> dispatch_time_t {
    delay
        .as_secs()
        .checked_mul(1_000_000_000)
        .and_then(|i| i.checked_add(delay.subsec_nanos() as u64))
        .and_then(|i| {
            if i < (i64::max_value() as u64) {
                Some(i as i64)
            } else {
                None
            }
        })
        .map_or(DISPATCH_TIME_FOREVER, |i| unsafe {
            dispatch_time(DISPATCH_TIME_NOW, i)
        })
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

/// Ref: https://github.com/apple-oss-distributions/libpthread/blob/c032e0b076700a0a47db75528a282b8d3a06531a/include/sys/qos.h#L130
#[derive(Debug, Copy, Clone)]
#[repr(u32)]
pub enum qos_class_t {
    QOS_CLASS_USER_INTERACTIVE = 0x21,
    QOS_CLASS_USER_INITIATED = 0x19,
    QOS_CLASS_DEFAULT = 0x15,
    QOS_CLASS_UTILITY = 0x11,
    QOS_CLASS_BACKGROUND = 0x09,
    QOS_CLASS_UNSPECIFIED = 0x00,
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

/// A Grand Central Dispatch queue.
///
/// For more information, see Apple's [Grand Central Dispatch reference](
/// https://developer.apple.com/library/mac/documentation/Performance/Reference/GCD_libdispatch_Ref/index.html).
#[derive(Debug)]
pub struct Queue {
    // FIX: This should not be public.
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

/// An RAII guard which will resume a suspended `Queue` when dropped.
#[derive(Debug)]
pub struct SuspendGuard {
    queue: Queue,
}

impl SuspendGuard {
    fn new(queue: &Queue) -> SuspendGuard {
        unsafe {
            dispatch_suspend(queue.ptr);
        }
        SuspendGuard {
            queue: queue.clone(),
        }
    }

    /// Drops self, allowing the suspended `Queue` to resume.
    #[allow(dead_code)]
    pub fn resume(self) {}
}

impl Clone for SuspendGuard {
    fn clone(&self) -> Self {
        SuspendGuard::new(&self.queue)
    }
}

impl Drop for SuspendGuard {
    fn drop(&mut self) {
        unsafe {
            dispatch_resume(self.queue.ptr);
        }
    }
}

use std::os::raw::c_long;

/// A counting semaphore.
#[derive(Debug)]
pub struct Semaphore {
    ptr: dispatch_semaphore_t,
}

impl Semaphore {
    /// Creates a new `Semaphore` with an initial value.
    ///
    /// A `Semaphore` created with a value greater than 0 cannot be disposed if
    /// it has been decremented below its original value. If there are more
    /// successful calls to `wait` than `signal`, the system assumes the
    /// `Semaphore` is still in use and will abort if it is disposed.
    pub fn new(value: u32) -> Self {
        let ptr = unsafe { dispatch_semaphore_create(value as c_long) };
        Semaphore { ptr }
    }

    /// Wait for (decrement) self.
    pub fn wait(&self) {
        let result = unsafe { dispatch_semaphore_wait(self.ptr, DISPATCH_TIME_FOREVER) };
        assert!(result == 0, "Dispatch semaphore wait errored");
    }

    /// Wait for (decrement) self until the specified timeout has elapsed.
    pub fn wait_timeout(&self, timeout: Duration) -> Result<(), WaitTimeout> {
        let when = time_after_delay(timeout);
        let result = unsafe { dispatch_semaphore_wait(self.ptr, when) };
        if result == 0 {
            Ok(())
        } else {
            Err(WaitTimeout { duration: timeout })
        }
    }

    /// Signal (increment) self.
    ///
    /// If the previous value was less than zero, this method wakes a waiting thread.
    /// Returns `true` if a thread is woken or `false` otherwise.
    pub fn signal(&self) -> bool {
        unsafe { dispatch_semaphore_signal(self.ptr) != 0 }
    }

    /// Wait to access a resource protected by self.
    /// This decrements self and returns a guard that increments when dropped.
    pub fn access(&self) -> SemaphoreGuard {
        self.wait();
        SemaphoreGuard::new(self.clone())
    }

    /// Wait until the specified timeout to access a resource protected by self.
    /// This decrements self and returns a guard that increments when dropped.
    pub fn access_timeout(&self, timeout: Duration) -> Result<SemaphoreGuard, WaitTimeout> {
        self.wait_timeout(timeout)?;
        Ok(SemaphoreGuard::new(self.clone()))
    }
}

unsafe impl Sync for Semaphore {}
unsafe impl Send for Semaphore {}

impl Clone for Semaphore {
    fn clone(&self) -> Self {
        unsafe {
            dispatch_retain(self.ptr);
        }
        Semaphore { ptr: self.ptr }
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe {
            dispatch_release(self.ptr);
        }
    }
}

/// An RAII guard which will signal a `Semaphore` when dropped.
#[derive(Debug)]
pub struct SemaphoreGuard {
    sem: Semaphore,
}

impl SemaphoreGuard {
    fn new(sem: Semaphore) -> SemaphoreGuard {
        SemaphoreGuard { sem }
    }

    /// Drops self, signaling the `Semaphore`.
    pub fn signal(self) {}
}

impl Drop for SemaphoreGuard {
    fn drop(&mut self) {
        self.sem.signal();
    }
}
