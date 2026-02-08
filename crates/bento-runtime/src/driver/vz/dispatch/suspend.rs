use super::{
    ffi::{dispatch_resume, dispatch_suspend},
    queue::Queue,
};

/// An RAII guard which will resume a suspended `Queue` when dropped.
#[derive(Debug)]
pub struct SuspendGuard {
    queue: Queue,
}

impl SuspendGuard {
    pub fn new(queue: &Queue) -> SuspendGuard {
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
