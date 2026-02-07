#![allow(non_snake_case)]
use objc2::__framework_prelude::*;
use objc2::{extern_class, extern_methods};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityElementProtocol, NSAnimatablePropertyContainer,
    NSAppearanceCustomization, NSDraggingDestination, NSResponder,
    NSUserInterfaceItemIdentification, NSView,
};
use objc2_foundation::*;
use objc2_virtualization::{
    VZConsoleDevice, VZDirectorySharingDevice, VZGraphicsDevice, VZMemoryBalloonDevice,
    VZNetworkDevice, VZSocketDevice, VZUSBController, VZVirtualMachineConfiguration,
    VZVirtualMachineDelegate, VZVirtualMachineStartOptions, VZVirtualMachineState,
};

use crate::dispatch::ffi::dispatch_queue_t;

extern_class!(
    #[unsafe(super(NSObject))]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct _VZVNCAuthenticationSecurityConfiguration;
);

impl _VZVNCAuthenticationSecurityConfiguration {
    extern_methods!(
        #[unsafe(method(new))]
        #[unsafe(method_family = new)]
        pub unsafe fn new() -> Retained<Self>;

        #[unsafe(method(init))]
        #[unsafe(method_family = init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        #[unsafe(method(initWithPassword:))]
        #[unsafe(method_family = init)]
        pub unsafe fn initWithPassword(
            this: Allocated<Self>,
            password: &NSString,
        ) -> Retained<Self>;

        #[unsafe(method(password))]
        #[unsafe(method_family = none)]
        pub unsafe fn password(&self) -> Retained<NSString>;
    );
}

unsafe impl NSObjectProtocol for _VZVNCAuthenticationSecurityConfiguration {}

extern_class!(
    #[unsafe(super(NSObject))]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct _VZVNCServer;
);

impl _VZVNCServer {
    extern_methods!(
        #[unsafe(method(new))]
        #[unsafe(method_family = new)]
        pub unsafe fn new() -> Retained<Self>;

        #[unsafe(method(init))]
        #[unsafe(method_family = init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        #[unsafe(method(initWithPort:queue:securityConfiguration:))]
        #[unsafe(method_family = init)]
        pub unsafe fn initWithPort_queue_securityConfiguration(
            this: Allocated<Self>,
            port: u16,
            queue: dispatch_queue_t,
            security_configuration: &_VZVNCAuthenticationSecurityConfiguration,
        ) -> Retained<Self>;

        #[unsafe(method(virtualMachine))]
        #[unsafe(method_family = none)]
        pub unsafe fn virtualMachine(&self) -> Option<Retained<VZVirtualMachine>>;

        #[unsafe(method(setVirtualMachine:))]
        #[unsafe(method_family = none)]
        pub unsafe fn setVirtualMachine(&self, virtual_machine: Option<&VZVirtualMachine>);

        #[unsafe(method(port))]
        #[unsafe(method_family = none)]
        pub unsafe fn port(&self) -> u16;

        #[unsafe(method(start))]
        #[unsafe(method_family = none)]
        pub unsafe fn start(&self);

        #[unsafe(method(stop))]
        #[unsafe(method_family = none)]
        pub unsafe fn stop(&self);
    );
}

unsafe impl NSObjectProtocol for _VZVNCServer {}

extern_class!(
    #[unsafe(super(NSObject))]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZMacOSInstaller;
);

unsafe impl NSObjectProtocol for VZMacOSInstaller {}

impl VZMacOSInstaller {
    extern_methods!(
        #[unsafe(method(new))]
        #[unsafe(method_family = new)]
        pub unsafe fn new() -> Retained<Self>;

        #[unsafe(method(init))]
        #[unsafe(method_family = init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        #[unsafe(method(initWithVirtualMachine:restoreImageURL:))]
        #[unsafe(method_family = init)]
        pub unsafe fn initWithVirtualMachine_restoreImageURL(
            this: Allocated<Self>,
            virtual_machine: &VZVirtualMachine,
            restore_image_file_url: &NSURL,
        ) -> Retained<Self>;

        /// Start installing macOS.
        ///
        /// Parameter `completionHandler`: Block called after installation has successfully completed or has failed.
        /// The error parameter passed to the block is nil if installation was successful. The block will be invoked on the virtual machine's queue.
        ///
        /// This method starts the installation process. The virtual machine must be in a stopped state. During the installation operation, pausing or stopping
        /// the virtual machine will result in undefined behavior.
        /// If installation is started on the same VZMacOSInstaller object more than once, an exception will be raised.
        /// This method must be called on the virtual machine's queue.
        #[unsafe(method(installWithCompletionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn installWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// An NSProgress object that can be used to observe or cancel installation.
        ///
        /// If the progress object is cancelled before installation is started, an exception will be raised.
        #[unsafe(method(progress))]
        #[unsafe(method_family = none)]
        pub unsafe fn progress(&self) -> Retained<NSProgress>;

        /// The virtual machine that this installer was initialized with.
        #[unsafe(method(virtualMachine))]
        #[unsafe(method_family = none)]
        pub unsafe fn virtualMachine(&self) -> Retained<VZVirtualMachine>;

        /// The restore image URL that this installer was initialized with.
        #[unsafe(method(restoreImageURL))]
        #[unsafe(method_family = none)]
        pub unsafe fn restoreImageURL(&self) -> Retained<NSURL>;
    );
}

extern_class!(
    #[unsafe(super(NSObject))]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZVirtualMachine;
);

unsafe impl NSObjectProtocol for VZVirtualMachine {}

impl VZVirtualMachine {
    extern_methods!(
        #[unsafe(method(new))]
        #[unsafe(method_family = new)]
        pub unsafe fn new() -> Retained<Self>;

        #[unsafe(method(init))]
        #[unsafe(method_family = init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        /// Initialize the virtual machine.
        ///
        /// This initializer uses the main queue to operate the virtual machine. Every call must be done on the main queue and the callbacks are invoked
        /// on the main queue.
        ///
        /// Parameter `configuration`: The configuration of the virtual machine.
        /// The configuration must be valid. Validation can be performed at runtime with [VZVirtualMachineConfiguration validateWithError:].
        /// The configuration is copied by the initializer.
        #[unsafe(method(initWithConfiguration:))]
        #[unsafe(method_family = init)]
        pub unsafe fn initWithConfiguration(
            this: Allocated<Self>,
            configuration: &VZVirtualMachineConfiguration,
        ) -> Retained<Self>;

        #[unsafe(method(initWithConfiguration:queue:))]
        #[unsafe(method_family = init)]
        pub unsafe fn initWithConfiguration_queue(
            this: Allocated<Self>,
            configuration: &VZVirtualMachineConfiguration,
            queue: dispatch_queue_t,
        ) -> Retained<Self>;

        /// Indicate whether or not virtualization is available.
        ///
        /// If virtualization is unavailable, no VZVirtualMachineConfiguration will validate.
        /// The validation error of the VZVirtualMachineConfiguration provides more information about why virtualization is unavailable.
        #[unsafe(method(isSupported))]
        #[unsafe(method_family = none)]
        pub unsafe fn isSupported() -> bool;

        /// Execution state of the virtual machine.
        #[unsafe(method(state))]
        #[unsafe(method_family = none)]
        pub unsafe fn state(&self) -> VZVirtualMachineState;

        /// The virtual machine delegate.
        #[unsafe(method(delegate))]
        #[unsafe(method_family = none)]
        pub unsafe fn delegate(
            &self,
        ) -> Option<Retained<ProtocolObject<dyn VZVirtualMachineDelegate>>>;

        /// This is a [weak property][objc2::topics::weak_property].
        /// Setter for [`delegate`][Self::delegate].
        #[unsafe(method(setDelegate:))]
        #[unsafe(method_family = none)]
        pub unsafe fn setDelegate(
            &self,
            delegate: Option<&ProtocolObject<dyn VZVirtualMachineDelegate>>,
        );

        /// Return YES if the machine is in a state that can be started.
        ///
        /// See: -[VZVirtualMachine startWithCompletionHandler:].
        ///
        /// See: -[VZVirtualMachine state]
        #[unsafe(method(canStart))]
        #[unsafe(method_family = none)]
        pub unsafe fn canStart(&self) -> bool;

        /// Return YES if the machine is in a state that can be stopped.
        ///
        /// See: -[VZVirtualMachine stopWithCompletionHandler:]
        ///
        /// See: -[VZVirtualMachine state]
        #[unsafe(method(canStop))]
        #[unsafe(method_family = none)]
        pub unsafe fn canStop(&self) -> bool;

        /// Return YES if the machine is in a state that can be paused.
        ///
        /// See: -[VZVirtualMachine pauseWithCompletionHandler:]
        ///
        /// See: -[VZVirtualMachine state]
        #[unsafe(method(canPause))]
        #[unsafe(method_family = none)]
        pub unsafe fn canPause(&self) -> bool;

        /// Return YES if the machine is in a state that can be resumed.
        ///
        /// See: -[VZVirtualMachine resumeWithCompletionHandler:]
        ///
        /// See: -[VZVirtualMachine state]
        #[unsafe(method(canResume))]
        #[unsafe(method_family = none)]
        pub unsafe fn canResume(&self) -> bool;

        /// Returns whether the machine is in a state where the guest can be asked to stop.
        ///
        /// See: -[VZVirtualMachine requestStopWithError:]
        ///
        /// See: -[VZVirtualMachine state]
        #[unsafe(method(canRequestStop))]
        #[unsafe(method_family = none)]
        pub unsafe fn canRequestStop(&self) -> bool;

        /// Return the list of console devices configured on this virtual machine. Return an empty array if no console device is configured.
        ///
        /// See: VZVirtioConsoleDeviceConfiguration
        ///
        /// See: VZVirtualMachineConfiguration
        #[unsafe(method(consoleDevices))]
        #[unsafe(method_family = none)]
        pub unsafe fn consoleDevices(&self) -> Retained<NSArray<VZConsoleDevice>>;

        /// Return the list of directory sharing devices configured on this virtual machine. Return an empty array if no directory sharing device is configured.
        ///
        /// See: VZVirtioFileSystemDeviceConfiguration
        ///
        /// See: VZVirtualMachineConfiguration
        #[unsafe(method(directorySharingDevices))]
        #[unsafe(method_family = none)]
        pub unsafe fn directorySharingDevices(&self)
            -> Retained<NSArray<VZDirectorySharingDevice>>;

        /// Return the list of graphics devices configured on this virtual machine. Return an empty array if no graphics device is configured.
        ///
        /// See: VZGraphicsDeviceConfiguration
        ///
        /// See: VZVirtualMachineConfiguration
        #[unsafe(method(graphicsDevices))]
        #[unsafe(method_family = none)]
        pub unsafe fn graphicsDevices(&self) -> Retained<NSArray<VZGraphicsDevice>>;

        /// Return the list of memory balloon devices configured on this virtual machine. Return an empty array if no memory balloon device is configured.
        ///
        /// See: VZVirtioTraditionalMemoryBalloonDeviceConfiguration
        ///
        /// See: VZVirtualMachineConfiguration
        #[unsafe(method(memoryBalloonDevices))]
        #[unsafe(method_family = none)]
        pub unsafe fn memoryBalloonDevices(&self) -> Retained<NSArray<VZMemoryBalloonDevice>>;

        /// Return the list of network devices configured on this virtual machine. Return an empty array if no network device is configured.
        ///
        /// See: VZVirtioNetworkDeviceConfiguration
        ///
        /// See: VZVirtualMachineConfiguration
        #[unsafe(method(networkDevices))]
        #[unsafe(method_family = none)]
        pub unsafe fn networkDevices(&self) -> Retained<NSArray<VZNetworkDevice>>;

        /// Return the list of socket devices configured on this virtual machine. Return an empty array if no socket device is configured.
        ///
        /// See: VZVirtioSocketDeviceConfiguration
        ///
        /// See: VZVirtualMachineConfiguration
        #[unsafe(method(socketDevices))]
        #[unsafe(method_family = none)]
        pub unsafe fn socketDevices(&self) -> Retained<NSArray<VZSocketDevice>>;

        /// Return the list of USB controllers configured on this virtual machine. Return an empty array if no USB controller is configured.
        ///
        /// See: VZUSBControllerConfiguration
        ///
        /// See: VZVirtualMachineConfiguration
        #[unsafe(method(usbControllers))]
        #[unsafe(method_family = none)]
        pub unsafe fn usbControllers(&self) -> Retained<NSArray<VZUSBController>>;

        /// Start a virtual machine.
        ///
        /// Start a virtual machine that is in either Stopped or Error state.
        ///
        /// Parameter `completionHandler`: Block called after the virtual machine has been successfully started or on error.
        /// The error parameter passed to the block is nil if the start was successful.
        #[unsafe(method(startWithCompletionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn startWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// Start a virtual machine with options.
        ///
        /// Start a virtual machine that is in either Stopped or Error state.
        ///
        /// Parameter `options`: Options used to control how the virtual machine is started.
        ///
        /// Parameter `completionHandler`: Block called after the virtual machine has been successfully started or on error.
        /// The error parameter passed to the block is nil if the start was successful.
        ///
        /// See also: VZMacOSVirtualMachineStartOptions
        #[unsafe(method(startWithOptions:completionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn startWithOptions_completionHandler(
            &self,
            options: &VZVirtualMachineStartOptions,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// Stop a virtual machine.
        ///
        /// Stop a virtual machine that is in either Running or Paused state.
        ///
        /// Parameter `completionHandler`: Block called after the virtual machine has been successfully stopped or on error.
        /// The error parameter passed to the block is nil if the stop was successful.
        ///
        /// This is a destructive operation. It stops the virtual machine without giving the guest a chance to stop cleanly.
        ///
        /// See also: -[VZVirtualMachine requestStopWithError:]
        #[unsafe(method(stopWithCompletionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn stopWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// Pause a virtual machine.
        ///
        /// Pause a virtual machine that is in Running state.
        ///
        /// Parameter `completionHandler`: Block called after the virtual machine has been successfully paused or on error.
        /// The error parameter passed to the block is nil if the pause was successful.
        #[unsafe(method(pauseWithCompletionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn pauseWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// Resume a virtual machine.
        ///
        /// Resume a virtual machine that is in the Paused state.
        ///
        /// Parameter `completionHandler`: Block called after the virtual machine has been successfully resumed or on error.
        /// The error parameter passed to the block is nil if the resumption was successful.
        #[unsafe(method(resumeWithCompletionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn resumeWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// Restore a virtual machine.
        ///
        /// Restore a stopped virtual machine to a state previously saved to file through `saveMachineStateToURL:completionHandler:`.
        ///
        /// If the file cannot be read, or contains otherwise invalid contents, this operation will fail with a `VZErrorRestore` error.
        /// If the virtual machine is not in the stopped state, this operation will fail with a `VZErrorInvalidVirtualMachineStateTransition` error.
        /// If the virtual machine cannot be started due to an internal error, this operation will fail with a `VZErrorInternal` error.
        /// The `VZVirtualMachineConfiguration` must also support restoring, which can be checked with  `-[VZVirtualMachineConfiguration validateSaveRestoreSupportWithError:]`.
        ///
        /// If this operation fails, the virtual machine state is unchanged.
        /// If successful, the virtual machine is restored and placed in the paused state.
        ///
        /// Parameter `saveFileURL`: URL to file containing saved state of a suspended virtual machine.
        /// The file must have been generated by `saveMachineStateToURL:completionHandler:` on the same host.
        /// Otherwise, this operation will fail with a `VZErrorRestore` error indicating a permission denied failure reason.
        ///
        /// The virtual machine must also be configured compatibly with the state contained in the file.
        /// If the `VZVirtualMachineConfiguration` is not compatible with the content of the file, this operation will fail with a `VZErrorRestore` error indicating an invalid argument failure reason.
        ///
        /// Files generated with `saveMachineStateToURL:completionHandler:` on a software version that is newer than the current version will also be rejected with an invalid argument failure reason.
        /// In some cases, `restoreMachineStateFromURL:completionHandler:` can fail if a software update has changed the host in a way that would be incompatible with the previous format.
        /// In this case, an invalid argument error will be surfaced. In most cases, the virtual machine should be restarted with `startWithCompletionHandler:`.
        ///
        /// Parameter `completionHandler`: Block called after the virtual machine has been successfully started or on error.
        /// The error parameter passed to the block is nil if the restore was successful.
        ///
        /// See: -[VZVirtualMachineConfiguration validateSaveRestoreSupportWithError:]
        #[unsafe(method(restoreMachineStateFromURL:completionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn restoreMachineStateFromURL_completionHandler(
            &self,
            save_file_url: &NSURL,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// Save a virtual machine.
        ///
        /// Save a paused virtual machine to file.
        /// The contents of this file can be used later to restore the state of the paused virtual machine.
        ///
        /// If the virtual machine is not paused, this operation will fail with a `VZErrorInvalidVirtualMachineState` error.
        /// If the virtual machine cannot be saved, this operation will fail with a `VZErrorSave` error.
        /// The `VZVirtualMachineConfiguration` must also support saving, which can be checked with  `-[VZVirtualMachineConfiguration validateSaveRestoreSupportWithError:]`.
        ///
        /// If this operation fails, the virtual machine state is unchanged.
        /// If successful, the file is written out and the virtual machine state is unchanged.
        ///
        /// Parameter `saveFileURL`: URL to location where the saved state of the virtual machine is to be written.
        /// Each file is protected by an encryption key that is tied to the host on which it is created.
        ///
        /// Parameter `completionHandler`: Block called after the virtual machine has been successfully saved or on error.
        /// The error parameter passed to the block is nil if the save was successful.
        ///
        /// See: -[VZVirtualMachineConfiguration validateSaveRestoreSupportWithError:]
        #[unsafe(method(saveMachineStateToURL:completionHandler:))]
        #[unsafe(method_family = none)]
        pub unsafe fn saveMachineStateToURL_completionHandler(
            &self,
            save_file_url: &NSURL,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        /// Request that the guest turns itself off.
        ///
        /// Parameter `error`: If not nil, assigned with the error if the request failed.
        ///
        /// Returns: YES if the request was made successfully.
        ///
        /// The -[VZVirtualMachineDelegate guestDidStopVirtualMachine:] delegate method is invoked when the guest has turned itself off.
        ///
        /// See also: -[VZVirtualMachineDelegate guestDidStopVirtualMachine:].
        #[unsafe(method(requestStopWithError:_))]
        #[unsafe(method_family = none)]
        pub unsafe fn requestStopWithError(&self) -> Result<(), Retained<NSError>>;
    );
}

extern_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZVirtualMachineView;
);

unsafe impl NSAccessibility for VZVirtualMachineView {}

unsafe impl NSAccessibilityElementProtocol for VZVirtualMachineView {}

unsafe impl NSAnimatablePropertyContainer for VZVirtualMachineView {}

unsafe impl NSAppearanceCustomization for VZVirtualMachineView {}

unsafe impl NSCoding for VZVirtualMachineView {}

unsafe impl NSDraggingDestination for VZVirtualMachineView {}

unsafe impl NSObjectProtocol for VZVirtualMachineView {}

unsafe impl NSUserInterfaceItemIdentification for VZVirtualMachineView {}

impl VZVirtualMachineView {
    extern_methods!(
        /// The virtual machine to display in the view.
        #[unsafe(method(virtualMachine))]
        #[unsafe(method_family = none)]
        pub unsafe fn virtualMachine(&self) -> Option<Retained<VZVirtualMachine>>;

        /// Setter for [`virtualMachine`][Self::virtualMachine].
        #[unsafe(method(setVirtualMachine:))]
        #[unsafe(method_family = none)]
        pub unsafe fn setVirtualMachine(&self, virtual_machine: Option<&VZVirtualMachine>);

        /// Whether certain system hot keys should be sent to the guest instead of the host. Defaults to NO.
        #[unsafe(method(capturesSystemKeys))]
        #[unsafe(method_family = none)]
        pub unsafe fn capturesSystemKeys(&self) -> bool;

        /// Setter for [`capturesSystemKeys`][Self::capturesSystemKeys].
        #[unsafe(method(setCapturesSystemKeys:))]
        #[unsafe(method_family = none)]
        pub unsafe fn setCapturesSystemKeys(&self, captures_system_keys: bool);

        /// Automatically reconfigures the graphics display associated with this view with respect to view changes. Defaults to NO.
        ///
        /// Automatically resize or reconfigure this graphics display when the view properties update.
        /// For example, resizing the display when the view has a live resize operation. When enabled,
        /// the graphics display will automatically be reconfigured to match the host display environment.
        ///
        /// This property can only be set on a single VZVirtualMachineView targeting a particular VZGraphicsDisplay
        /// at a time. If multiple VZVirtualMachineViews targeting the same VZGraphicsDisplay enable this property,
        /// only one view will respect the property, and the other view will have had the property disabled.
        #[unsafe(method(automaticallyReconfiguresDisplay))]
        #[unsafe(method_family = none)]
        pub unsafe fn automaticallyReconfiguresDisplay(&self) -> bool;

        /// Setter for [`automaticallyReconfiguresDisplay`][Self::automaticallyReconfiguresDisplay].
        #[unsafe(method(setAutomaticallyReconfiguresDisplay:))]
        #[unsafe(method_family = none)]
        pub unsafe fn setAutomaticallyReconfiguresDisplay(
            &self,
            automatically_reconfigures_display: bool,
        );
    );
}

/// Methods declared on superclass `NSView`.
impl VZVirtualMachineView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        #[unsafe(method_family = init)]
        pub unsafe fn initWithFrame(this: Allocated<Self>, frame_rect: NSRect) -> Retained<Self>;

        #[unsafe(method(initWithCoder:))]
        #[unsafe(method_family = init)]
        pub unsafe fn initWithCoder(
            this: Allocated<Self>,
            coder: &NSCoder,
        ) -> Option<Retained<Self>>;
    );
}

/// Methods declared on superclass `NSResponder`.
impl VZVirtualMachineView {
    extern_methods!(
        #[unsafe(method(init))]
        #[unsafe(method_family = init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;
    );
}

/// Methods declared on superclass `NSObject`.
impl VZVirtualMachineView {
    extern_methods!(
        #[unsafe(method(new))]
        #[unsafe(method_family = new)]
        pub unsafe fn new(mtm: MainThreadMarker) -> Retained<Self>;
    );
}
