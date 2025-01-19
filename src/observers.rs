use std::{ffi::c_void, ptr};

use objc2::{
    declare_class, msg_send_id, mutability, rc::Retained, runtime::AnyObject, ClassType,
    DeclaredClass,
};
use objc2_foundation::{
    ns_string, NSCopying, NSDictionary, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNumber,
    NSObject, NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSProgress, NSString,
};

pub(crate) struct Ivars {
    object: Retained<NSProgress>,
    key_path: Retained<NSString>,
    handler: Box<dyn Fn(Option<f64>) + 'static>,
}

declare_class!(
    #[derive(Debug)]
    pub(crate) struct NSProgressFractionCompletedObserver;

    unsafe impl ClassType for NSProgressFractionCompletedObserver {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "NSProgressFractionCompletedObserver";
    }

    impl DeclaredClass for NSProgressFractionCompletedObserver {
        type Ivars = Ivars;
    }

    unsafe impl NSProgressFractionCompletedObserver {
        #[method(observeValueForKeyPath:ofObject:change:context:)]
        unsafe fn observe_value_for_key_path(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            if let Some(change) = change {
                let state = change.get(&NSString::from_str("new"));
                let ptr: *const AnyObject = state.unwrap();
                let value: *const NSNumber = ptr.cast();
                let p = value.as_ref().unwrap_unchecked();
                (self.ivars().handler)(Some(p.as_f64()));
            } else {
                (self.ivars().handler)(None);
            }
        }
    }

    unsafe impl NSObjectProtocol for NSProgressFractionCompletedObserver {}
);

impl NSProgressFractionCompletedObserver {
    pub fn new(
        object: Retained<NSProgress>,
        handler: impl Fn(Option<f64>) + 'static + Send + Sync,
    ) -> Retained<Self> {
        let options = NSKeyValueObservingOptions::NSKeyValueObservingOptionNew;
        let key_path = ns_string!("fractionCompleted");
        let observer = Self::alloc().set_ivars(Ivars {
            object,
            key_path: key_path.copy(),
            handler: Box::new(handler),
        });
        let observer: Retained<Self> = unsafe { msg_send_id![super(observer), init] };

        // SAFETY: We make sure to un-register the observer before it's deallocated.
        //
        // Passing `NULL` as the `context` parameter here is fine, as the observer does not
        // have any subclasses, and the superclass (NSObject) is not observing anything.
        unsafe {
            observer
                .ivars()
                .object
                .addObserver_forKeyPath_options_context(
                    &observer,
                    key_path,
                    options,
                    ptr::null_mut(),
                );
        }

        observer
    }
}

impl Drop for NSProgressFractionCompletedObserver {
    fn drop(&mut self) {
        unsafe {
            self.ivars()
                .object
                .removeObserver_forKeyPath(&self, &self.ivars().key_path);
        }
    }
}
