use std::{
    ffi::c_void,
    fmt::{Debug, Display},
    fs,
    io::ErrorKind,
    ptr,
    sync::{mpsc::sync_channel, Arc, Mutex},
};

use block2::StackBlock;
use crossbeam::channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, AllocAnyThread, DefinedClass,
};
use objc2_foundation::{
    ns_string, NSCopying, NSData, NSDictionary, NSError, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSNumber, NSObject, NSObjectNSKeyValueObserverRegistration,
    NSObjectProtocol, NSString,
};

use crate::{
    driver::{vz::vz::VZVirtualMachineExt, DriverError},
    instance::{Instance, InstanceFile},
};

use super::dispatch::{Queue, QueueAttribute};
use objc2_virtualization::{
    VZGenericMachineIdentifier, VZVirtualMachine, VZVirtualMachineConfiguration,
    VZVirtualMachineState,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualMachineState {
    Stopped = 0,
    Running = 1,
    Paused = 2,
    Error = 3,
    Starting = 4,
    Pausing = 5,
    Resuming = 6,
    Stopping = 7,
    Saving = 8,
    Restoring = 9,
    Unknown = -1,
}

impl Default for VirtualMachineState {
    fn default() -> Self {
        VirtualMachineState::Unknown
    }
}

impl Display for VirtualMachineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VirtualMachineState::Stopped => write!(f, "Stopped"),
            VirtualMachineState::Running => write!(f, "Running"),
            VirtualMachineState::Paused => write!(f, "Paused"),
            VirtualMachineState::Error => write!(f, "Error"),
            VirtualMachineState::Starting => write!(f, "Starting"),
            VirtualMachineState::Pausing => write!(f, "Pausing"),
            VirtualMachineState::Resuming => write!(f, "Resuming"),
            VirtualMachineState::Stopping => write!(f, "Stopping"),
            VirtualMachineState::Saving => write!(f, "Saving"),
            VirtualMachineState::Restoring => write!(f, "Restoring"),
            VirtualMachineState::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VirtualMachine {
    queue: Queue,
    machine: Retained<VZVirtualMachine>,
    _config: Retained<VZVirtualMachineConfiguration>,
    _observer: Retained<VirtualMachineStateObserver>,
    current_state: Arc<Mutex<VirtualMachineState>>,
    state_subscribers: Arc<Mutex<Vec<StateSubscriber>>>,
}

type StateSubscriber = (Sender<VirtualMachineState>, Receiver<VirtualMachineState>);

#[derive(Debug, Clone, Error)]
#[error("{description} [domain={domain}, code={code}]{failure_reason}{recovery_suggestion}")]
pub struct VirtualMachineError {
    pub domain: String,
    pub code: isize,
    pub description: String,
    pub failure_reason: String,
    pub recovery_suggestion: String,
}

impl VirtualMachineError {
    fn from_nserror(error: &NSError) -> Self {
        let failure_reason = unsafe {
            error
                .localizedFailureReason()
                .map(|reason| format!(", reason={reason}"))
                .unwrap_or_default()
        };
        let recovery_suggestion = unsafe {
            error
                .localizedRecoverySuggestion()
                .map(|suggestion| format!(", suggestion={suggestion}"))
                .unwrap_or_default()
        };

        Self {
            domain: error.domain().to_string(),
            code: error.code(),
            description: error.localizedDescription().to_string(),
            failure_reason,
            recovery_suggestion,
        }
    }

    fn completion_channel_closed(context: &'static str) -> Self {
        Self {
            domain: "rust.channel".to_string(),
            code: -1,
            description: format!("{context} completion channel closed before result was delivered"),
            failure_reason: String::new(),
            recovery_suggestion: String::new(),
        }
    }
}

impl VirtualMachine {
    pub fn new(config: Retained<VZVirtualMachineConfiguration>) -> Self {
        unsafe {
            let queue = Queue::create("codes.nvd.bentobox", QueueAttribute::Serial);

            let machine = VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &config,
                queue.ptr,
            );

            let current_state = Arc::new(Mutex::new(map_virtual_machine_state(machine.state())));
            let state_subscribers: Arc<Mutex<Vec<StateSubscriber>>> =
                Arc::new(Mutex::new(Vec::new()));

            let observer_current_state = current_state.clone();
            let observer_subscribers = state_subscribers.clone();

            let observer = VirtualMachineStateObserver::new(machine.clone(), move |change| {
                let state = change.objectForKey(ns_string!("new"));
                let p = state.unwrap().downcast::<NSNumber>().unwrap();
                let state = VZVirtualMachineState(p.as_isize());
                let state_msg = map_virtual_machine_state(state);

                {
                    let mut current = observer_current_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *current = state_msg;
                }

                let mut subscribers = observer_subscribers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                subscribers.retain(|(tx, rx)| try_send_latest(tx, rx, state_msg));
            });

            return VirtualMachine {
                queue,
                machine,
                _config: config,
                _observer: observer,
                current_state,
                state_subscribers,
            };
        }
    }

    pub fn start(&self) -> Result<(), VirtualMachineError> {
        let machine = self.machine.clone();
        let (sender, receiver) = sync_channel(0);

        let completion_handler = StackBlock::new(move |err: *mut NSError| {
            let err = unsafe { err.as_ref() };
            let result = match err {
                Some(error) => Err(VirtualMachineError::from_nserror(error)),
                None => Ok(()),
            };

            let _ = sender.send(result);
        });
        self.queue
            .exec_block_async(&StackBlock::new(move || unsafe {
                machine.startWithCompletionHandler(&completion_handler);
            }));

        receiver
            .recv()
            .map_err(|_| VirtualMachineError::completion_channel_closed("start"))?
    }

    pub fn stop(&self) -> Result<(), VirtualMachineError> {
        let machine = self.machine.clone();
        let (sender, receiver) = sync_channel(0);

        let completion_handler = StackBlock::new(move |err: *mut NSError| {
            let err = unsafe { err.as_ref() };
            let result = match err {
                Some(error) => Err(VirtualMachineError::from_nserror(error)),
                None => Ok(()),
            };

            let _ = sender.send(result);
        });

        self.queue
            .exec_block_async(&StackBlock::new(move || unsafe {
                machine.stopWithCompletionHandler(&completion_handler);
            }));

        receiver
            .recv()
            .map_err(|_| VirtualMachineError::completion_channel_closed("stop"))?
    }

    #[expect(dead_code, reason = "kept for upcoming VM control surface")]
    pub fn can_start(&self) -> bool {
        let machine = self.machine.clone();
        self.run_on_queue(move || unsafe { machine.canStart() })
    }

    #[expect(dead_code, reason = "kept for upcoming VM control surface")]
    pub fn can_stop(&self) -> bool {
        let machine = self.machine.clone();
        self.run_on_queue(move || unsafe { machine.canStop() })
    }

    #[allow(unused)]
    pub fn can_pause(&self) -> bool {
        let machine = self.machine.clone();
        self.run_on_queue(move || unsafe { machine.canPause() })
    }

    #[allow(unused)]
    pub fn can_resume(&self) -> bool {
        let machine = self.machine.clone();
        self.run_on_queue(move || unsafe { machine.canResume() })
    }

    #[allow(unused)]
    pub fn can_request_stop(&self) -> bool {
        let machine = self.machine.clone();

        self.run_on_queue(move || unsafe { machine.canRequestStop() })
    }

    #[expect(dead_code, reason = "kept for upcoming VM control surface")]
    pub fn request_stop(&self) -> Result<(), VirtualMachineError> {
        let machine = self.machine.clone();

        self.run_on_queue(move || match unsafe { machine.requestStopWithError() } {
            Ok(()) => Ok(()),
            Err(error) => Err(VirtualMachineError::from_nserror(&error)),
        })
    }

    pub fn state(&self) -> VirtualMachineState {
        let machine = self.machine.clone();

        self.run_on_queue(move || map_virtual_machine_state(unsafe { machine.state() }))
    }

    fn run_on_queue<F, R>(&self, cb: F) -> R
    where
        F: 'static + Fn() -> R + Clone,
        R: 'static,
    {
        let result = Arc::new(Mutex::new(None));
        let block = StackBlock::new({
            let result = result.clone();
            move || {
                let mut slot = result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *slot = Some(cb());
            }
        });

        self.queue.exec_block_sync(&block);

        let mut slot = result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot.take() {
            Some(value) => value,
            None => panic!("queue callback completed without producing a value"),
        }
    }

    pub fn subscribe_state(&self) -> Receiver<VirtualMachineState> {
        let (tx, rx) = bounded(1);

        let snapshot = {
            let state = self
                .current_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *state
        };

        {
            let mut subscribers = self
                .state_subscribers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            subscribers.push((tx.clone(), rx.clone()));
        }

        let _ = try_send_latest(&tx, &rx, snapshot);

        rx
    }
}

fn map_virtual_machine_state(state: VZVirtualMachineState) -> VirtualMachineState {
    match state {
        VZVirtualMachineState::Starting => VirtualMachineState::Starting,
        VZVirtualMachineState::Running => VirtualMachineState::Running,
        VZVirtualMachineState::Saving => VirtualMachineState::Saving,
        VZVirtualMachineState::Error => VirtualMachineState::Error,
        VZVirtualMachineState::Pausing => VirtualMachineState::Pausing,
        VZVirtualMachineState::Paused => VirtualMachineState::Paused,
        VZVirtualMachineState::Resuming => VirtualMachineState::Resuming,
        VZVirtualMachineState::Stopping => VirtualMachineState::Stopping,
        VZVirtualMachineState::Stopped => VirtualMachineState::Stopped,
        VZVirtualMachineState::Restoring => VirtualMachineState::Restoring,
        VZVirtualMachineState(_) => VirtualMachineState::Unknown,
    }
}

fn try_send_latest(
    tx: &Sender<VirtualMachineState>,
    rx: &Receiver<VirtualMachineState>,
    state: VirtualMachineState,
) -> bool {
    match tx.try_send(state) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            match rx.try_recv() {
                Ok(_) | Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return false,
            }

            match tx.try_send(state) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            }
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

struct Ivars {
    object: Retained<VZVirtualMachine>,
    key_path: Retained<NSString>,
    handler: Box<dyn Fn(&NSDictionary<NSKeyValueChangeKey, AnyObject>) + Send + Sync + 'static>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - MyObserver implements `Drop` and ensures that:
    //   - It does not call an overridden method.
    //   - It does not `retain` itself.
    #[unsafe(super(NSObject))]
    #[name = "VirtualMachineStateObserver"]
    #[ivars = Ivars]
    struct VirtualMachineStateObserver;

    impl VirtualMachineStateObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        unsafe fn observe_value_for_key_path(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            if let Some(change) = change {
                (self.ivars().handler)(change);
            } else {
                (self.ivars().handler)(&NSDictionary::new());
            }
        }
    }

    unsafe impl NSObjectProtocol for VirtualMachineStateObserver {}
);

impl VirtualMachineStateObserver {
    fn new(
        object: Retained<VZVirtualMachine>,
        // key_path: &NSString,
        // options: NSKeyValueObservingOptions,
        // TODO: Thread safety? This probably depends on whether the observed
        // object is later moved to another thread.
        handler: impl Fn(&NSDictionary<NSKeyValueChangeKey, AnyObject>) + 'static + Send + Sync,
    ) -> Retained<Self> {
        let options = NSKeyValueObservingOptions::New;
        let key_path = ns_string!("state");
        let observer = Self::alloc().set_ivars(Ivars {
            object,
            key_path: key_path.copy(),
            handler: Box::new(handler),
        });
        let observer: Retained<Self> = unsafe { msg_send![super(observer), init] };

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

impl Debug for VirtualMachineStateObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let object_ptr: *const VZVirtualMachine = &*self.ivars().object;
        f.debug_struct("VirtualMachineStateObserver")
            .field("__superclass", &self.__superclass)
            .field("object", &format_args!("{object_ptr:p}"))
            .field("key_path", &"state")
            .finish()
    }
}

impl Drop for VirtualMachineStateObserver {
    fn drop(&mut self) {
        unsafe {
            self.ivars()
                .object
                .removeObserver_forKeyPath(&self, &self.ivars().key_path);
        }
    }
}

pub fn get_machine_identifier(
    inst: &Instance,
) -> Result<Retained<VZGenericMachineIdentifier>, DriverError> {
    let identifier_path = inst.file(InstanceFile::AppleMachineIdentifier);

    let needs_new_identifier = match fs::metadata(&identifier_path) {
        Ok(meta) => meta.len() == 0,
        Err(err) if err.kind() == ErrorKind::NotFound => true,
        Err(_) => {
            return Err(DriverError::Backend(format!(
                "stat machine identifier file {}",
                identifier_path.display()
            )));
        }
    };

    if needs_new_identifier {
        let machine_identifier = unsafe { VZGenericMachineIdentifier::new() };
        let data = unsafe { machine_identifier.dataRepresentation() };

        fs::write(&identifier_path, data.to_vec())?;

        return Ok(machine_identifier);
    }

    let bytes = fs::read(&identifier_path)?;
    let data = NSData::with_bytes(&bytes);

    let maybe_machine_identifier = unsafe {
        VZGenericMachineIdentifier::initWithDataRepresentation(
            VZGenericMachineIdentifier::alloc(),
            &data,
        )
    };

    let Some(machine_identifier) = maybe_machine_identifier else {
        return Err(DriverError::Backend(format!(
            "stat machine identifier file {}",
            identifier_path.display()
        )));
    };

    Ok(machine_identifier)
}
