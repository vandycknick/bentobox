use std::{
    cell::Cell,
    ffi::c_void,
    fmt::Display,
    os::fd::AsRawFd,
    ptr,
    sync::{mpsc::sync_channel, Arc},
};

use block2::StackBlock;
use crossbeam::channel::{bounded, Receiver};
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    AllocAnyThread, ClassType, DeclaredClass,
};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSCopying, NSData, NSDataBase64DecodingOptions,
    NSDataBase64EncodingOptions, NSDictionary, NSError, NSFileHandle, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSNumber, NSObject, NSObjectNSKeyValueObserverRegistration,
    NSObjectProtocol, NSString, NSURL,
};
use objc2_virtualization::{
    VZDiskImageStorageDeviceAttachment, VZFileHandleSerialPortAttachment, VZLinuxBootLoader,
    VZMACAddress, VZMacAuxiliaryStorage, VZMacAuxiliaryStorageInitializationOptions,
    VZMacGraphicsDeviceConfiguration, VZMacGraphicsDisplayConfiguration, VZMacHardwareModel,
    VZMacMachineIdentifier, VZMacOSBootLoader, VZMacPlatformConfiguration,
    VZNATNetworkDeviceAttachment, VZUSBKeyboardConfiguration,
    VZUSBScreenCoordinatePointingDeviceConfiguration, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioNetworkDeviceConfiguration, VZVirtioTraditionalMemoryBalloonDeviceConfiguration,
    VZVirtualMachineConfiguration, VZVirtualMachineState,
};

use crate::{
    dispatch::{Queue, QueueAttribute},
    internal::{VZMacOSInstaller, VZVirtualMachine},
    window::AppDelegate,
};

use crate::observers::NSProgressFractionCompletedObserver;

#[derive(Debug, Clone)]
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

#[derive(Clone)]
pub struct VirtualMachine {
    queue: Queue,
    pub machine: Retained<VZVirtualMachine>,
    observer: Retained<VirtualMachineStateObserver>,
    state_notifications: Receiver<VirtualMachineState>,
}

impl VirtualMachine {
    pub fn supported() -> bool {
        unsafe { VZVirtualMachine::isSupported() }
    }

    pub fn new(config: Retained<VZVirtualMachineConfiguration>) -> Self {
        unsafe {
            let queue = Queue::create("codes.nvd.bentobox", QueueAttribute::Serial);
            let (sender, receiver) = bounded(0);

            let machine = VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &config,
                queue.ptr,
            );

            let observer = VirtualMachineStateObserver::new(machine.clone(), move |change| {
                let state = change.objectForKey(ns_string!("new"));
                let p = state.unwrap().downcast::<NSNumber>().unwrap();

                //let ptr: *const AnyObject = state.unwrap();
                //let value: *const NSNumber = ptr.cast();
                //let p = value.as_ref().unwrap_unchecked();
                // TODO: wrap this in a custom type so that the internal VZ unsafe doesn't leak
                let state = VZVirtualMachineState(p.as_isize());
                let state_msg = match state {
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
                };

                println!("Machine is {}", state_msg);

                // TODO: let's not ignore this error
                match sender.try_send(state_msg) {
                    Ok(_) => println!("Message got send correctly!"),
                    Err(e) => eprintln!("Couldn't send the message: {}!", e),
                }
            });

            return VirtualMachine {
                queue,
                machine,
                observer,
                state_notifications: receiver,
            };
        }
    }

    pub fn start(&self) -> Result<(), String> {
        let machine = self.machine.clone();
        let (sender, receiver) = sync_channel(0);

        let completion_handler = StackBlock::new(move |err: *mut NSError| {
            let err = unsafe { err.as_ref() };
            match err {
                Some(error) => sender
                    .send(Err(error.localizedDescription().to_string()))
                    .unwrap(),
                None => sender.send(Ok(())).unwrap(),
            }
        });
        self.queue
            .exec_block_async(&StackBlock::new(move || unsafe {
                machine.startWithCompletionHandler(&completion_handler);
                //machine.startWithOptions_completionHandler(options, completion_handler);
            }));

        receiver
            .recv()
            .expect("failed to receive from machine start completion handler")
    }

    pub fn stop(&self) -> Result<(), String> {
        let machine = self.machine.clone();
        let (sender, receiver) = sync_channel(0);

        let completion_handler = StackBlock::new(move |err: *mut NSError| {
            let err = unsafe { err.as_ref() };
            match err {
                Some(error) => sender
                    .send(Err(error.localizedDescription().to_string()))
                    .unwrap(),
                None => sender.send(Ok(())).unwrap(),
            }
        });

        self.queue
            .exec_block_async(&StackBlock::new(move || unsafe {
                machine.stopWithCompletionHandler(&completion_handler);
            }));

        receiver
            .recv()
            .expect("failed to receive from machine stop completion handler")
    }

    pub fn open_window(&self) {
        let mtm = MainThreadMarker::new().unwrap();

        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        //configure the application delegate
        let delegate = AppDelegate::new(mtm, self.clone());
        let object = ProtocolObject::from_ref(&*delegate);
        app.setDelegate(Some(object));

        // run the app
        app.run();
    }

    pub fn can_start(&self) -> bool {
        let machine = self.machine.clone();
        self.run_on_queue(move || unsafe { machine.canStart() })
    }

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

    pub fn request_stop(&self) -> Result<(), String> {
        let machine = self.machine.clone();
        let result = self.run_on_queue(move || {
            let result = unsafe { machine.requestStopWithError() };

            match result {
                Err(_) => 1,
                _ => 0,
            }
        });

        match result {
            1 => Err("Something went wrong!".to_string()),
            _ => Ok(()),
        }
    }

    pub fn state(&self) -> VirtualMachineState {
        let machine = self.machine.clone();

        self.run_on_queue(move || {
            let state = unsafe { machine.state() };
            return match state {
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
            };
        })
    }

    fn run_on_queue<F, R>(&self, cb: F) -> R
    where
        F: 'static + Fn() -> R + Clone,
        R: Default + Clone + 'static,
    {
        let result = Arc::new(Cell::new(R::default()));
        let block = StackBlock::new({
            let result = result.clone();
            move || {
                result.replace(cb());
            }
        });

        self.queue.exec_block_sync(&block);
        result.take()
    }

    pub fn install_macos(&self, restore_image: impl AsRef<str>) {
        unsafe {
            let machine = self.machine.clone();

            let (tx, rx) = std::sync::mpsc::channel();
            let url = NSString::from_str(restore_image.as_ref());
            let restore_url = NSURL::initFileURLWithPath(NSURL::alloc(), &url);

            self.run_on_queue(move || {
                let tx_inner = tx.clone();
                let installer = VZMacOSInstaller::initWithVirtualMachine_restoreImageURL(
                    VZMacOSInstaller::alloc(),
                    machine.as_ref(),
                    &restore_url,
                );

                let progress = installer.progress();
                let observer = NSProgressFractionCompletedObserver::new(progress.clone(), |f| {
                    if let Some(p) = f {
                        println!("Progress: {:.1}%.", p * 100.0);
                    }
                });
                // thread::spawn(move || loop {
                //     let completion = progress.fractionCompleted();
                //     println!("[thread] Progress: {:.1}%.", completion * 100.0);
                //     thread::sleep(Duration::from_secs(1));
                // });

                let handler = StackBlock::new(move |e: *mut NSError| {
                    let _ = observer.clone();
                    match e.as_ref() {
                        Some(error) => {
                            println!("Installation failed with: {}", error.localizedDescription())
                        }
                        None => println!("Installation finished!"),
                    }
                    tx_inner.send(1).unwrap();
                });

                println!("Progress: {:.1}%.", 0.0 * 100.0);
                installer.installWithCompletionHandler(&handler);
            });

            match rx.recv() {
                Ok(r) => println!("This is what I got back over the channel: {}", r),
                Err(e) => eprintln!("WTF: {}", e),
            }
        }
    }

    pub fn get_state_channel(&self) -> &Receiver<VirtualMachineState> {
        &self.state_notifications
    }
}
// NOTE: This should be safe as long as all vm operations are done via dispatch.
unsafe impl Send for VirtualMachine {}
unsafe impl Sync for VirtualMachine {}

struct Ivars {
    object: Retained<VZVirtualMachine>,
    key_path: Retained<NSString>,
    handler: Box<dyn Fn(&NSDictionary<NSKeyValueChangeKey, AnyObject>) + 'static>,
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

impl Drop for VirtualMachineStateObserver {
    fn drop(&mut self) {
        unsafe {
            self.ivars()
                .object
                .removeObserver_forKeyPath(&self, &self.ivars().key_path);
        }
    }
}

pub struct VirtualMachineBuilder {
    config: Retained<VZVirtualMachineConfiguration>,
}

pub enum VirtualMachineGuestPlatform {
    Linux {
        kernel: String,
        initramfs: String,
        command_line: Option<String>,
    },
}

impl VirtualMachineBuilder {
    pub fn new() -> Self {
        VirtualMachineBuilder::default()
    }

    pub fn use_cpus(self, cpus: usize) -> Self {
        unsafe {
            self.config.setCPUCount(cpus);
        }
        self
    }

    pub fn use_memory(self, memory: u64) -> Self {
        unsafe {
            self.config.setMemorySize(memory);
        }

        self
    }

    pub fn use_storage_device(self, device_path: impl AsRef<str>) -> Self {
        unsafe {
            let url = NSString::from_str(device_path.as_ref());
            let device = NSURL::initFileURLWithPath(NSURL::alloc(), &url);
            // TODO: fix unwrap here.
            let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                VZDiskImageStorageDeviceAttachment::alloc(),
                &device,
                false,
            )
            .unwrap();
            let block_config = VZVirtioBlockDeviceConfiguration::initWithAttachment(
                VZVirtioBlockDeviceConfiguration::alloc(),
                &attachment,
            );

            let c = [block_config.as_super()];

            let devices = NSArray::from_slice(&c);
            self.config.setStorageDevices(&devices);
        }
        self
    }

    pub fn use_network(self) -> Self {
        unsafe {
            let nat_attachment = VZNATNetworkDeviceAttachment::new();
            let network_device = VZVirtioNetworkDeviceConfiguration::new();
            let mac = VZMACAddress::randomLocallyAdministeredAddress();
            network_device.setAttachment(Some(&nat_attachment));
            network_device.setMACAddress(&mac);

            let devices = NSArray::from_slice(&[network_device.as_super()]);
            self.config.setNetworkDevices(&devices);
        }
        // let network_device = vz::VirtioNetworkDeviceConfiguration::new_with_attachment(
        //     vz::NATNetworkDeviceAttachment::new(),
        // );
        // network_device.set_mac_address(vz::MACAddress::new_with_random_locally_administered_address());
        self
    }

    pub fn use_keyboard(self) -> Self {
        unsafe {
            let keyboard = VZUSBKeyboardConfiguration::new();
            let keyboards = NSArray::from_slice(&[keyboard.as_super()]);
            self.config.setKeyboards(&keyboards);
        }
        self
    }

    pub fn use_platform(self, platform: VirtualMachineGuestPlatform) -> Self {
        match platform {
            VirtualMachineGuestPlatform::Linux {
                kernel,
                initramfs,
                command_line,
            } => unsafe {
                let bootloader = VZLinuxBootLoader::new();

                let kernel = NSString::from_str(&kernel);
                let kernel_url = NSURL::initFileURLWithPath(NSURL::alloc(), &kernel);
                bootloader.setKernelURL(&kernel_url);

                let initramfs = NSString::from_str(&initramfs);
                let initramfs_url = NSURL::initFileURLWithPath(NSURL::alloc(), &initramfs);
                bootloader.setInitialRamdiskURL(Some(&initramfs_url));

                let command_line = command_line
                    .unwrap_or("console=hvc0 rd.break=initqueue root=/dev/vda".to_string());
                bootloader.setCommandLine(&NSString::from_str(&command_line));

                self.config.setBootLoader(Some(&bootloader));
            },
        }

        self
    }

    pub fn use_platform_macos(
        self,
        aux_storage: impl AsRef<str>,
        machine_id: Option<&str>,
    ) -> Self {
        unsafe {
            let platform = VZMacPlatformConfiguration::new();
            let hw_data = NSData::initWithBase64EncodedString_options(
                NSData::alloc(),
                ns_string!("YnBsaXN0MDDTAQIDBAUGXxAZRGF0YVJlcHJlc2VudGF0aW9uVmVyc2lvbl8QD1BsYXRmb3JtVmVyc2lvbl8QEk1pbmltdW1TdXBwb3J0ZWRPUxQAAAAAAAAAAAAAAAAAAAABEAKjBwgIEAwQAAgPKz1SY2VpawAAAAAAAAEBAAAAAAAAAAkAAAAAAAAAAAAAAAAAAABt"),
                NSDataBase64DecodingOptions::IgnoreUnknownCharacters
            );

            let hw_model = VZMacHardwareModel::initWithDataRepresentation(
                VZMacHardwareModel::alloc(),
                &hw_data.unwrap_or(NSData::new()),
            )
            .unwrap();

            let machine_id = if let Some(id) = machine_id {
                let hw_data = NSData::initWithBase64EncodedString_options(
                    NSData::alloc(),
                    &NSString::from_str(id),
                    NSDataBase64DecodingOptions::IgnoreUnknownCharacters,
                );
                let id = VZMacMachineIdentifier::initWithDataRepresentation(
                    VZMacMachineIdentifier::alloc(),
                    &hw_data.unwrap_or(NSData::new()),
                );

                //TODO: fixme
                id.unwrap()
            } else {
                VZMacMachineIdentifier::new()
            };

            let url = NSString::from_str(aux_storage.as_ref());
            let aux_url = NSURL::initFileURLWithPath(NSURL::alloc(), &url);

            //https://developer.apple.com/documentation/virtualization/vzmacauxiliarystorage/3816043-initwithcontentsofurl?language=objc
            // let aux_storage = VZMacAuxiliaryStorage::initWithContentsOfURL(this, url)
            let aux = VZMacAuxiliaryStorage::initCreatingStorageAtURL_hardwareModel_options_error(
                VZMacAuxiliaryStorage::alloc(),
                &aux_url,
                &hw_model,
                VZMacAuxiliaryStorageInitializationOptions::AllowOverwrite,
            );

            let bootloader = VZMacOSBootLoader::new();

            platform.setAuxiliaryStorage(aux.ok().as_deref());
            platform.setHardwareModel(&hw_model);
            platform.setMachineIdentifier(&machine_id);

            // TODO: find a way to store this in config. We need the ecid to boot the mac in the
            // next step.
            let machine_id_data = machine_id.dataRepresentation();
            let machine_id_str =
                machine_id_data.base64EncodedStringWithOptions(NSDataBase64EncodingOptions(0));
            println!("Machine Id: {}", machine_id_str.to_string());

            self.config.setPlatform(&platform);
            self.config.setBootLoader(Some(&bootloader));
        }
        self
    }

    pub fn use_graphics_device(self) -> Self {
        unsafe {
            let graphics_device = VZMacGraphicsDeviceConfiguration::new();
            let display = VZMacGraphicsDisplayConfiguration::initWithWidthInPixels_heightInPixels_pixelsPerInch(
                VZMacGraphicsDisplayConfiguration::alloc(),
                2560,
                1600,
                200
            );

            let displays = NSArray::from_slice(&[display.as_ref()]);
            graphics_device.setDisplays(&displays);

            let devices = NSArray::from_slice(&[graphics_device.as_super()]);
            self.config.setGraphicsDevices(&devices);

            // TODO: this doens't belong here:
            let pointer = VZUSBScreenCoordinatePointingDeviceConfiguration::new();
            let devices = NSArray::from_slice(&[pointer.as_super()]);

            self.config.setPointingDevices(&devices);
        }
        self
    }

    pub fn use_memory_balloon(self) -> Self {
        unsafe {
            let balloon = VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new();
            let devices = NSArray::from_slice(&[balloon.as_super()]);
            self.config.setMemoryBalloonDevices(&devices);
        }
        self
    }

    pub fn use_entropy_device(self) -> Self {
        unsafe {
            let entropy = VZVirtioEntropyDeviceConfiguration::new();
            let devices = NSArray::from_slice(&[entropy.as_super()]);
            self.config.setEntropyDevices(&devices);
        }
        self
    }

    pub fn use_console<T: AsRawFd, U: AsRawFd>(
        self,
        stdin: Option<&T>,
        stdout: Option<&U>,
    ) -> Self {
        unsafe {
            let stdin = stdin.map(|s| {
                NSFileHandle::initWithFileDescriptor(NSFileHandle::alloc(), s.as_raw_fd())
            });
            let stdout = stdout.map(|s| {
                NSFileHandle::initWithFileDescriptor(NSFileHandle::alloc(), s.as_raw_fd())
            });
            let attachment =
                VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                    VZFileHandleSerialPortAttachment::alloc(),
                    stdin.as_deref(),
                    stdout.as_deref(),
                );

            let serial_port = VZVirtioConsoleDeviceSerialPortConfiguration::new();
            serial_port.setAttachment(Some(&attachment));

            let ports = NSArray::from_slice(&[serial_port.as_super()]);

            self.config.setSerialPorts(&ports);
        }

        self
    }

    pub fn build(&self) -> VirtualMachine {
        let result = unsafe { self.config.validateWithError() };

        if let Err(k) = &result {
            let msg = k.localizedDescription();

            println!("Invalid VirtualizationConfiguration: {}", msg);

            // TODO: Fix this Let it panic!
            //let _ = result.unwrap();
        }

        println!("{:?}", result);

        VirtualMachine::new(self.config.clone())
    }
}

impl Default for VirtualMachineBuilder {
    fn default() -> Self {
        VirtualMachineBuilder {
            config: unsafe { VZVirtualMachineConfiguration::new() },
        }
    }
}
