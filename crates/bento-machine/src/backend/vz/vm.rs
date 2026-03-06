use std::{
    ffi::c_void,
    fmt::{Debug, Display},
    fs,
    io::ErrorKind,
    os::fd::{BorrowedFd, OwnedFd},
    path::Path,
    ptr,
    sync::{mpsc::sync_channel, Arc, Mutex},
};

use block2::StackBlock;
use crossbeam::channel::{bounded, Receiver, Sender};
use nix::unistd::dup;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, AllocAnyThread, DefinedClass,
};
use objc2_foundation::{
    ns_string, NSData, NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSNumber, NSObject, NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString,
};
use objc2_virtualization::{
    VZGenericMachineIdentifier, VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtualMachine,
    VZVirtualMachineConfiguration, VZVirtualMachineState,
};
use thiserror::Error;

use crate::types::{MachineError, MachineState};

use super::dispatch::{Queue, QueueAttribute};
use super::objc_ext::VZVirtualMachineExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    #[default]
    Unknown = -1,
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
    serial_guest_input: Arc<OwnedFd>,
    serial_guest_output: Arc<OwnedFd>,
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

impl From<VirtualMachineError> for MachineError {
    fn from(value: VirtualMachineError) -> Self {
        MachineError::Backend(value.to_string())
    }
}

impl VirtualMachine {
    pub fn new(
        config: Retained<VZVirtualMachineConfiguration>,
        serial_guest_input: OwnedFd,
        serial_guest_output: OwnedFd,
    ) -> Self {
        unsafe {
            let queue = Queue::create("codes.nvd.bentobox.machine", QueueAttribute::Serial);
            let machine = VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &config,
                queue.ptr,
            );

            let current_state = Arc::new(Mutex::new(map_virtual_machine_state(machine.state())));
            let state_subscribers = Arc::new(Mutex::new(Vec::new()));

            let observer_current_state = current_state.clone();
            let observer_subscribers = state_subscribers.clone();
            let observer = VirtualMachineStateObserver::new(machine.clone(), move |change| {
                let state = change.objectForKey(ns_string!("new"));
                let Some(number) = state.and_then(|value| value.downcast::<NSNumber>().ok()) else {
                    return;
                };
                let state = VZVirtualMachineState(number.as_isize());
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

            Self {
                queue,
                machine,
                _config: config,
                _observer: observer,
                current_state,
                state_subscribers,
                serial_guest_input: Arc::new(serial_guest_input),
                serial_guest_output: Arc::new(serial_guest_output),
            }
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

    pub fn open_vsock_stream(&self, port: u32) -> Result<OwnedFd, VirtualMachineError> {
        let machine = self.machine.clone();
        let (sender, receiver) = sync_channel(0);

        self.queue
            .exec_block_async(&StackBlock::new(move || unsafe {
                let devices = machine.socketDevices();
                let Some(device) = devices.firstObject() else {
                    let _ = sender.send(Err(virtual_machine_error(
                        "no socket device configured in VM",
                    )));
                    return;
                };

                let Some(vsock) = device.downcast_ref::<VZVirtioSocketDevice>() else {
                    let _ = sender.send(Err(virtual_machine_error(
                        "socket device is not a virtio socket device",
                    )));
                    return;
                };

                let completion_sender = sender.clone();
                let completion_handler = StackBlock::new(
                    move |connection: *mut VZVirtioSocketConnection, err: *mut NSError| {
                        let err = err.as_ref();
                        if let Some(error) = err {
                            let _ = completion_sender
                                .send(Err(VirtualMachineError::from_nserror(error)));
                            return;
                        }

                        let Some(connection) = connection.as_ref() else {
                            let _ = completion_sender.send(Err(virtual_machine_error(
                                "vsock connection completed without a connection object",
                            )));
                            return;
                        };

                        let file_descriptor = connection.fileDescriptor();
                        let borrowed = BorrowedFd::borrow_raw(file_descriptor);
                        let result = dup(borrowed).map_err(|err| {
                            virtual_machine_error(&format!(
                                "duplicate vsock file descriptor: {err}"
                            ))
                        });
                        let _ = completion_sender.send(result);
                    },
                );

                vsock.connectToPort_completionHandler(port, &completion_handler);
            }));

        receiver
            .recv()
            .map_err(|_| VirtualMachineError::completion_channel_closed("open_vsock_stream"))?
    }

    pub fn open_serial_fds(&self) -> Result<(OwnedFd, OwnedFd), VirtualMachineError> {
        let input = dup(&*self.serial_guest_input).map_err(|err| {
            virtual_machine_error(&format!("duplicate serial guest input fd: {err}"))
        })?;
        let output = dup(&*self.serial_guest_output).map_err(|err| {
            virtual_machine_error(&format!("duplicate serial guest output fd: {err}"))
        })?;
        Ok((input, output))
    }

    pub fn state(&self) -> VirtualMachineState {
        self.current_state
            .lock()
            .map(|state| *state)
            .unwrap_or_default()
    }

    pub fn subscribe_state(&self) -> Receiver<VirtualMachineState> {
        let (tx, rx) = bounded(16);
        let current = self.state();
        let _ = tx.send(current);
        let mut subscribers = self
            .state_subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        subscribers.push((tx, rx.clone()));
        rx
    }
}

impl From<VirtualMachineState> for MachineState {
    fn from(value: VirtualMachineState) -> Self {
        match value {
            VirtualMachineState::Running => MachineState::Running,
            VirtualMachineState::Stopped => MachineState::Stopped,
            _ => MachineState::Created,
        }
    }
}

fn map_virtual_machine_state(state: VZVirtualMachineState) -> VirtualMachineState {
    match state.0 {
        0 => VirtualMachineState::Stopped,
        1 => VirtualMachineState::Running,
        2 => VirtualMachineState::Paused,
        3 => VirtualMachineState::Error,
        4 => VirtualMachineState::Starting,
        5 => VirtualMachineState::Pausing,
        6 => VirtualMachineState::Resuming,
        7 => VirtualMachineState::Stopping,
        8 => VirtualMachineState::Saving,
        9 => VirtualMachineState::Restoring,
        _ => VirtualMachineState::Unknown,
    }
}

fn try_send_latest(
    tx: &Sender<VirtualMachineState>,
    rx: &Receiver<VirtualMachineState>,
    state: VirtualMachineState,
) -> bool {
    while !rx.is_empty() {
        let _ = rx.recv();
    }
    tx.send(state).is_ok()
}

fn virtual_machine_error(description: &str) -> VirtualMachineError {
    VirtualMachineError {
        domain: "bento.machine.vz".to_string(),
        code: -1,
        description: description.to_string(),
        failure_reason: String::new(),
        recovery_suggestion: String::new(),
    }
}

struct Ivars {
    object: Retained<VZVirtualMachine>,
    key_path: Retained<NSString>,
    handler: ObserverHandler,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BentoMachineVirtualMachineStateObserver"]
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

type ObserverHandler =
    Box<dyn Fn(&NSDictionary<NSKeyValueChangeKey, AnyObject>) + Send + Sync + 'static>;

impl VirtualMachineStateObserver {
    fn new(
        object: Retained<VZVirtualMachine>,
        handler: impl Fn(&NSDictionary<NSKeyValueChangeKey, AnyObject>) + Send + Sync + 'static,
    ) -> Retained<Self> {
        let options = NSKeyValueObservingOptions::New;
        let key_path = ns_string!("state");
        let observer = Self::alloc().set_ivars(Ivars {
            object,
            key_path: NSString::from_str("state"),
            handler: Box::new(handler),
        });
        let observer: Retained<Self> = unsafe { msg_send![super(observer), init] };

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
                .removeObserver_forKeyPath(self, &self.ivars().key_path);
        }
    }
}

pub fn get_machine_identifier(
    path: &Path,
) -> Result<Retained<VZGenericMachineIdentifier>, MachineError> {
    let needs_new_identifier = match fs::metadata(path) {
        Ok(meta) => meta.len() == 0,
        Err(err) if err.kind() == ErrorKind::NotFound => true,
        Err(err) => {
            return Err(MachineError::Backend(format!(
                "stat machine identifier file {}: {err}",
                path.display()
            )));
        }
    };

    if needs_new_identifier {
        let machine_identifier = unsafe { VZGenericMachineIdentifier::new() };
        let data = unsafe { machine_identifier.dataRepresentation() };
        fs::write(path, data.to_vec())?;
        return Ok(machine_identifier);
    }

    let bytes = fs::read(path)?;
    let data = NSData::with_bytes(&bytes);
    let maybe_machine_identifier = unsafe {
        VZGenericMachineIdentifier::initWithDataRepresentation(
            VZGenericMachineIdentifier::alloc(),
            &data,
        )
    };

    maybe_machine_identifier.ok_or_else(|| {
        MachineError::Backend(format!("load machine identifier file {}", path.display()))
    })
}
