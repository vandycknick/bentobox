use super::ffi::{dispatch_resume, dispatch_suspend};
use super::Queue;

#[derive(Debug)]
pub struct SuspendGuard {
    queue: Queue,
}

impl SuspendGuard {
    pub(crate) fn new(queue: &Queue) -> Self {
        unsafe { dispatch_suspend(queue.ptr) };
        Self {
            queue: queue.clone(),
        }
    }
}

impl Drop for SuspendGuard {
    fn drop(&mut self) {
        unsafe { dispatch_resume(self.queue.ptr) };
    }
}
