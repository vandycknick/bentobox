use std::ffi::c_void;
use std::fmt::{Debug, Display};
use std::sync::{Arc, Mutex};

use block2::StackBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, AllocAnyThread, DefinedClass,
};
use objc2_foundation::{
    ns_string, NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSNumber,
    NSObject, NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString,
};
use objc2_virtualization::{
    VZVirtualMachine, VZVirtualMachineConfiguration, VZVirtualMachineState,
};
use tokio::sync::oneshot;

use crate::configuration::VirtualMachineConfiguration;
use crate::device::{
    EntropyDeviceConfiguration, MemoryBalloonDeviceConfiguration, NetworkDeviceConfiguration,
    SerialPortConfiguration, SocketDeviceConfiguration, StorageDeviceConfiguration,
    VirtioFileSystemDeviceConfiguration, VirtioSocketDevice,
};
use crate::dispatch::{Queue, QueueAttribute};
use crate::error::VzError;
use crate::vz_ext::VZVirtualMachineExt;
use crate::{GenericPlatform, LinuxBootLoader};

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
}

impl Display for VirtualMachineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "Stopped"),
            Self::Running => write!(f, "Running"),
            Self::Paused => write!(f, "Paused"),
            Self::Error => write!(f, "Error"),
            Self::Starting => write!(f, "Starting"),
            Self::Pausing => write!(f, "Pausing"),
            Self::Resuming => write!(f, "Resuming"),
            Self::Stopping => write!(f, "Stopping"),
            Self::Saving => write!(f, "Saving"),
            Self::Restoring => write!(f, "Restoring"),
        }
    }
}

impl From<VZVirtualMachineState> for VirtualMachineState {
    fn from(value: VZVirtualMachineState) -> Self {
        match value.0 {
            0 => Self::Stopped,
            1 => Self::Running,
            2 => Self::Paused,
            3 => Self::Error,
            4 => Self::Starting,
            5 => Self::Pausing,
            6 => Self::Resuming,
            7 => Self::Stopping,
            8 => Self::Saving,
            9 => Self::Restoring,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VirtualMachine {
    queue: Queue,
    machine: Retained<VZVirtualMachine>,
    // NOTE: This may not need to live on VirtualMachine. We keep a retained configuration here
    // because the original implementation was conservative about the Objective-C lifetime after
    // VM initialization. VZVirtualMachine likely retains the configuration internally, so this
    // field may be removable after verifying the VM remains stable when the Rust-side config drops.
    _config: Retained<VZVirtualMachineConfiguration>,
    _observer: Retained<VirtualMachineStateObserver>,
    current_state: Arc<Mutex<VirtualMachineState>>,
}

pub struct VirtualMachineBuilder {
    config: VirtualMachineConfiguration,
}

// SAFETY: Every Virtualization.framework interaction goes through the VM's serial dispatch queue,
// or reads cached state maintained by callbacks that also run on that queue.
unsafe impl Send for VirtualMachine {}
// SAFETY: See above. The queue is the synchronization boundary, not the calling thread.
unsafe impl Sync for VirtualMachine {}

impl VirtualMachine {
    pub fn builder() -> Result<VirtualMachineBuilder, VzError> {
        Ok(VirtualMachineBuilder {
            config: VirtualMachineConfiguration::new()?,
        })
    }

    pub(crate) fn from_parts(
        queue: Queue,
        machine: Retained<VZVirtualMachine>,
        config: Retained<VZVirtualMachineConfiguration>,
    ) -> Self {
        let current_state = Arc::new(Mutex::new(unsafe { machine.state().into() }));
        let observer_current_state = current_state.clone();
        let observer = VirtualMachineStateObserver::new(machine.clone(), move |change| {
            let state = change.objectForKey(ns_string!("new"));
            let Some(number) = state.and_then(|value| value.downcast::<NSNumber>().ok()) else {
                return;
            };
            let state = VZVirtualMachineState(number.as_isize());
            let state_msg: VirtualMachineState = state.into();

            let mut current = observer_current_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *current = state_msg;
        });

        Self {
            queue,
            machine,
            _config: config,
            _observer: observer,
            current_state,
        }
    }

    pub async fn start(&self) -> Result<(), VzError> {
        let machine = self.machine.clone();
        let (sender, receiver) = oneshot::channel();
        let shared_sender = Arc::new(Mutex::new(Some(sender)));
        let completion_sender = shared_sender.clone();

        self.queue
            .exec_block_async(&StackBlock::new(move || unsafe {
                let completion_sender = completion_sender.clone();
                let completion_handler = StackBlock::new(move |err: *mut NSError| {
                    let err = err.as_ref();
                    let result = match err {
                        Some(error) => {
                            Err(VzError::Backend(error.localizedDescription().to_string()))
                        }
                        None => Ok(()),
                    };
                    if let Some(sender) = completion_sender
                        .lock()
                        .ok()
                        .and_then(|mut guard| guard.take())
                    {
                        let _ = sender.send(result);
                    }
                });

                machine.startWithCompletionHandler(&completion_handler);
            }));

        receiver.await.map_err(|_| {
            VzError::Backend(
                "start completion channel closed before result was delivered".to_string(),
            )
        })?
    }

    pub async fn stop(&self) -> Result<(), VzError> {
        let machine = self.machine.clone();
        let (sender, receiver) = oneshot::channel();
        let shared_sender = Arc::new(Mutex::new(Some(sender)));
        let completion_sender = shared_sender.clone();

        self.queue
            .exec_block_async(&StackBlock::new(move || unsafe {
                let completion_sender = completion_sender.clone();
                let completion_handler = StackBlock::new(move |err: *mut NSError| {
                    let err = err.as_ref();
                    let result = match err {
                        Some(error) => {
                            Err(VzError::Backend(error.localizedDescription().to_string()))
                        }
                        None => Ok(()),
                    };
                    if let Some(sender) = completion_sender
                        .lock()
                        .ok()
                        .and_then(|mut guard| guard.take())
                    {
                        let _ = sender.send(result);
                    }
                });

                machine.stopWithCompletionHandler(&completion_handler);
            }));

        receiver.await.map_err(|_| {
            VzError::Backend(
                "stop completion channel closed before result was delivered".to_string(),
            )
        })?
    }

    pub fn state(&self) -> VirtualMachineState {
        self.current_state
            .lock()
            .map(|state| *state)
            .unwrap_or_default()
    }

    pub fn open_devices(&self) -> Result<Vec<VirtioSocketDevice>, VzError> {
        let count = Arc::new(Mutex::new(0usize));
        let count_out = count.clone();
        let machine = self.machine.clone();
        self.queue.exec_block_sync(&StackBlock::new(move || unsafe {
            let devices = machine.socketDevices();
            let mut count = count_out
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *count = devices.count();
        }));

        let count = *count
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok((0..count)
            .map(|index| VirtioSocketDevice::new(self.machine.clone(), self.queue.clone(), index))
            .collect())
    }
}

impl VirtualMachineBuilder {
    pub fn set_cpu_count(mut self, cpu_count: usize) -> Self {
        self.config.set_cpu_count(cpu_count);
        self
    }

    pub fn set_memory_size(mut self, memory_size_bytes: u64) -> Self {
        self.config.set_memory_size(memory_size_bytes);
        self
    }

    pub fn set_platform(mut self, platform: GenericPlatform) -> Self {
        self.config.set_platform(platform);
        self
    }

    pub fn set_boot_loader(mut self, boot_loader: LinuxBootLoader) -> Self {
        self.config.set_boot_loader(boot_loader);
        self
    }

    pub fn add_entropy_device(mut self, device: EntropyDeviceConfiguration) -> Self {
        self.config.add_entropy_device(device);
        self
    }

    pub fn add_memory_balloon_device(mut self, device: MemoryBalloonDeviceConfiguration) -> Self {
        self.config.add_memory_balloon_device(device);
        self
    }

    pub fn add_network_device(mut self, device: NetworkDeviceConfiguration) -> Self {
        self.config.add_network_device(device);
        self
    }

    pub fn add_serial_port(mut self, port: SerialPortConfiguration) -> Self {
        self.config.add_serial_port(port);
        self
    }

    pub fn add_socket_device(mut self, device: SocketDeviceConfiguration) -> Self {
        self.config.add_socket_device(device);
        self
    }

    pub fn add_storage_device(mut self, device: StorageDeviceConfiguration) -> Self {
        self.config.add_storage_device(device);
        self
    }

    pub fn add_directory_share(mut self, device: VirtioFileSystemDeviceConfiguration) -> Self {
        self.config.add_directory_share(device);
        self
    }

    pub fn build(self) -> Result<VirtualMachine, VzError> {
        let machine_config = self.config.build()?;

        unsafe {
            let queue = Queue::create("codes.nvd.bentobox.vz.machine", QueueAttribute::Serial);
            let machine = VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &machine_config,
                queue.ptr,
            );
            Ok(VirtualMachine::from_parts(queue, machine, machine_config))
        }
    }
}

struct Ivars {
    object: Retained<VZVirtualMachine>,
    key_path: Retained<NSString>,
    handler: ObserverHandler,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BentoVzVirtualMachineStateObserver"]
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
                    std::ptr::null_mut(),
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
