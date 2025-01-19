#![allow(non_snake_case)]
use objc2::__framework_prelude::*;
use objc2::{extern_class, extern_methods, ClassType};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityElementProtocol, NSAnimatablePropertyContainer,
    NSAppearanceCustomization, NSDraggingDestination, NSResponder,
    NSUserInterfaceItemIdentification, NSView,
};
use objc2_foundation::*;
use objc2_virtualization::{
    VZConsoleDevice, VZDirectorySharingDevice, VZGraphicsDevice, VZMemoryBalloonDevice,
    VZNetworkDevice, VZSocketDevice, VZVirtualMachineConfiguration, VZVirtualMachineDelegate,
    VZVirtualMachineStartOptions, VZVirtualMachineState,
};

use crate::queue::dispatch_queue_t;

extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct _VZVNCAuthenticationSecurityConfiguration;

    unsafe impl ClassType for _VZVNCAuthenticationSecurityConfiguration {
        type Super = NSObject;
        type Mutability = InteriorMutable;
    }
);

extern_methods!(
    unsafe impl _VZVNCAuthenticationSecurityConfiguration {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new() -> Retained<Self>;

        #[method_id(@__retain_semantics Init init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        #[method_id(@__retain_semantics Init initWithPassword:)]
        pub unsafe fn initWithPassword(
            this: Allocated<Self>,
            password: &NSString,
        ) -> Retained<Self>;

        #[method_id(@__retain_semantics Other password)]
        pub unsafe fn password(&self) -> Retained<NSString>;
    }
);

unsafe impl NSObjectProtocol for _VZVNCAuthenticationSecurityConfiguration {}

extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct _VZVNCServer;

    unsafe impl ClassType for _VZVNCServer {
        type Super = NSObject;
        type Mutability = InteriorMutable;
    }
);

extern_methods!(
    unsafe impl _VZVNCServer {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new() -> Retained<Self>;

        #[method_id(@__retain_semantics Init init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        #[method_id(@__retain_semantics Init initWithPort:queue:securityConfiguration:)]
        pub unsafe fn initWithPort_queue_securityConfiguration(
            this: Allocated<Self>,
            port: u16,
            queue: dispatch_queue_t,
            security_configuration: &_VZVNCAuthenticationSecurityConfiguration,
        ) -> Retained<Self>;

        #[method_id(@__retain_semantics Other virtualMachine)]
        pub unsafe fn virtualMachine(&self) -> Option<Retained<VZVirtualMachine>>;

        #[method(setVirtualMachine:)]
        pub unsafe fn setVirtualMachine(&self, virtual_machine: Option<&VZVirtualMachine>);

        #[method(port)]
        pub unsafe fn port(&self) -> u16;

        #[method(start)]
        pub unsafe fn start(&self);

        #[method(stop)]
        pub unsafe fn stop(&self);
    }
);

unsafe impl NSObjectProtocol for _VZVNCServer {}

extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZMacOSInstaller;

    unsafe impl ClassType for VZMacOSInstaller {
        type Super = NSObject;
        type Mutability = InteriorMutable;
    }
);

unsafe impl NSObjectProtocol for VZMacOSInstaller {}

extern_methods!(
    unsafe impl VZMacOSInstaller {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new() -> Retained<Self>;

        #[method_id(@__retain_semantics Init init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        #[method_id(@__retain_semantics Init initWithVirtualMachine:restoreImageURL:)]
        pub unsafe fn initWithVirtualMachine_restoreImageURL(
            this: Allocated<Self>,
            virtual_machine: &VZVirtualMachine,
            restore_image_file_url: &NSURL,
        ) -> Retained<Self>;

        #[method(installWithCompletionHandler:)]
        pub unsafe fn installWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method_id(@__retain_semantics Other progress)]
        pub unsafe fn progress(&self) -> Retained<NSProgress>;

        #[method_id(@__retain_semantics Other virtualMachine)]
        pub unsafe fn virtualMachine(&self) -> Retained<VZVirtualMachine>;

        #[method_id(@__retain_semantics Other restoreImageURL)]
        pub unsafe fn restoreImageURL(&self) -> Retained<NSURL>;
    }
);

extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZVirtualMachine;

    unsafe impl ClassType for VZVirtualMachine {
        type Super = NSObject;
        type Mutability = InteriorMutable;
    }
);

unsafe impl NSObjectProtocol for VZVirtualMachine {}

extern_methods!(
    unsafe impl VZVirtualMachine {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new() -> Retained<Self>;

        #[method_id(@__retain_semantics Init init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;

        #[method_id(@__retain_semantics Init initWithConfiguration:)]
        pub unsafe fn initWithConfiguration(
            this: Allocated<Self>,
            configuration: &VZVirtualMachineConfiguration,
        ) -> Retained<Self>;

        #[method_id(@__retain_semantics Init initWithConfiguration:queue:)]
        pub unsafe fn initWithConfiguration_queue(
            this: Allocated<Self>,
            configuration: &VZVirtualMachineConfiguration,
            queue: dispatch_queue_t,
        ) -> Retained<Self>;

        #[method(isSupported)]
        pub unsafe fn isSupported() -> bool;

        #[method(state)]
        pub unsafe fn state(&self) -> VZVirtualMachineState;

        #[method_id(@__retain_semantics Other delegate)]
        pub unsafe fn delegate(
            &self,
        ) -> Option<Retained<ProtocolObject<dyn VZVirtualMachineDelegate>>>;

        #[method(setDelegate:)]
        pub unsafe fn setDelegate(
            &self,
            delegate: Option<&ProtocolObject<dyn VZVirtualMachineDelegate>>,
        );

        #[method(canStart)]
        pub unsafe fn canStart(&self) -> bool;

        #[method(canStop)]
        pub unsafe fn canStop(&self) -> bool;

        #[method(canPause)]
        pub unsafe fn canPause(&self) -> bool;

        #[method(canResume)]
        pub unsafe fn canResume(&self) -> bool;

        #[method(canRequestStop)]
        pub unsafe fn canRequestStop(&self) -> bool;

        #[method_id(@__retain_semantics Other consoleDevices)]
        pub unsafe fn consoleDevices(&self) -> Retained<NSArray<VZConsoleDevice>>;

        #[method_id(@__retain_semantics Other directorySharingDevices)]
        pub unsafe fn directorySharingDevices(&self)
            -> Retained<NSArray<VZDirectorySharingDevice>>;

        #[method_id(@__retain_semantics Other graphicsDevices)]
        pub unsafe fn graphicsDevices(&self) -> Retained<NSArray<VZGraphicsDevice>>;

        #[method_id(@__retain_semantics Other memoryBalloonDevices)]
        pub unsafe fn memoryBalloonDevices(&self) -> Retained<NSArray<VZMemoryBalloonDevice>>;

        #[method_id(@__retain_semantics Other networkDevices)]
        pub unsafe fn networkDevices(&self) -> Retained<NSArray<VZNetworkDevice>>;

        #[method_id(@__retain_semantics Other socketDevices)]
        pub unsafe fn socketDevices(&self) -> Retained<NSArray<VZSocketDevice>>;

        #[method(startWithCompletionHandler:)]
        pub unsafe fn startWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method(startWithOptions:completionHandler:)]
        pub unsafe fn startWithOptions_completionHandler(
            &self,
            options: &VZVirtualMachineStartOptions,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method(stopWithCompletionHandler:)]
        pub unsafe fn stopWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method(pauseWithCompletionHandler:)]
        pub unsafe fn pauseWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method(resumeWithCompletionHandler:)]
        pub unsafe fn resumeWithCompletionHandler(
            &self,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method(restoreMachineStateFromURL:completionHandler:)]
        pub unsafe fn restoreMachineStateFromURL_completionHandler(
            &self,
            save_file_url: &NSURL,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method(saveMachineStateToURL:completionHandler:)]
        pub unsafe fn saveMachineStateToURL_completionHandler(
            &self,
            save_file_url: &NSURL,
            completion_handler: &block2::Block<dyn Fn(*mut NSError)>,
        );

        #[method(requestStopWithError:_)]
        pub unsafe fn requestStopWithError(&self) -> Result<(), Retained<NSError>>;
    }
);

extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZVirtualMachineView;

    unsafe impl ClassType for VZVirtualMachineView {
        #[inherits(NSResponder, NSObject)]
        type Super = NSView;
        type Mutability = MainThreadOnly;
    }
);

unsafe impl NSAccessibility for VZVirtualMachineView {}

unsafe impl NSAccessibilityElementProtocol for VZVirtualMachineView {}

unsafe impl NSAnimatablePropertyContainer for VZVirtualMachineView {}

unsafe impl NSAppearanceCustomization for VZVirtualMachineView {}

unsafe impl NSCoding for VZVirtualMachineView {}

unsafe impl NSDraggingDestination for VZVirtualMachineView {}

unsafe impl NSObjectProtocol for VZVirtualMachineView {}

unsafe impl NSUserInterfaceItemIdentification for VZVirtualMachineView {}

extern_methods!(
    unsafe impl VZVirtualMachineView {
        #[method_id(@__retain_semantics Other virtualMachine)]
        pub unsafe fn virtualMachine(&self) -> Option<Retained<VZVirtualMachine>>;

        #[method(setVirtualMachine:)]
        pub unsafe fn setVirtualMachine(&self, virtual_machine: Option<&VZVirtualMachine>);

        #[method(capturesSystemKeys)]
        pub unsafe fn capturesSystemKeys(&self) -> bool;

        #[method(setCapturesSystemKeys:)]
        pub unsafe fn setCapturesSystemKeys(&self, captures_system_keys: bool);

        #[method(automaticallyReconfiguresDisplay)]
        pub unsafe fn automaticallyReconfiguresDisplay(&self) -> bool;

        #[method(setAutomaticallyReconfiguresDisplay:)]
        pub unsafe fn setAutomaticallyReconfiguresDisplay(
            &self,
            automatically_reconfigures_display: bool,
        );
    }
);

extern_methods!(
    /// Methods declared on superclass `NSView`
    unsafe impl VZVirtualMachineView {
        #[method_id(@__retain_semantics Init initWithFrame:)]
        pub unsafe fn initWithFrame(this: Allocated<Self>, frame_rect: NSRect) -> Retained<Self>;

        #[method_id(@__retain_semantics Init initWithCoder:)]
        pub unsafe fn initWithCoder(
            this: Allocated<Self>,
            coder: &NSCoder,
        ) -> Option<Retained<Self>>;
    }
);

extern_methods!(
    /// Methods declared on superclass `NSResponder`
    unsafe impl VZVirtualMachineView {
        #[method_id(@__retain_semantics Init init)]
        pub unsafe fn init(this: Allocated<Self>) -> Retained<Self>;
    }
);

extern_methods!(
    unsafe impl VZVirtualMachineView {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new(mtm: MainThreadMarker) -> Retained<Self>;
    }
);
