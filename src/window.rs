use crate::internal::{VZVirtualMachine, VZVirtualMachineView};
use crate::vm::{VirtualMachine, VirtualMachineState};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, ClassType, DeclaredClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationDelegate, NSBackingStoreType,
    NSMenu, NSMenuItem, NSRunningApplication, NSWindow, NSWindowDelegate, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSSize,
};

pub struct Ivars {
    mtm: MainThreadMarker,
    vm: VirtualMachine,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `AppDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppDelegate"]
    #[ivars = Ivars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, notification: &NSNotification) {
            println!("Did finish launching!");
            self.setup_menu_bar();
            self.setup_window();
            dbg!(notification);
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            self.try_shutdown_vm();
            println!("Will terminate!");
        }
    }

    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, notification: &NSNotification) {
            println!("Window will close!");
            dbg!(notification);
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker, vm: VirtualMachine) -> Retained<Self> {
        let this = mtm.alloc();
        let this = this.set_ivars(Ivars { vm, mtm });
        unsafe { msg_send![super(this), init] }
    }

    pub fn setup_menu_bar(&self) {
        unsafe {
            let mtm = self.ivars().mtm;
            let menu_bar = NSMenu::initWithTitle(mtm.alloc(), ns_string!(""));
            let menu_bar_item = NSMenuItem::new(mtm);

            menu_bar.addItem(&menu_bar_item);

            let app = NSApplication::sharedApplication(mtm);
            app.setMainMenu(Some(&menu_bar));

            // NSMenu *menuBar = [[[NSMenu alloc] init] autorelease];
            // NSMenuItem *menuBarItem = [[[NSMenuItem alloc] init] autorelease];
            // [menuBar addItem:menuBarItem];
            // [NSApp setMainMenu:menuBar];
        }
    }

    pub fn try_shutdown_vm(&self) {
        let vm = self.ivars().vm.clone();

        if !vm.can_request_stop() {
            println!("Can't request stop");
            return;
        }

        vm.stop().unwrap();

        let mut cnt = 0;
        loop {
            cnt += 1;

            if cnt > 10 {
                println!("VM took longer than 10 seconds to stop, exiting now!");
                break;
            }

            std::thread::sleep(std::time::Duration::from_secs(1));
            println!("Shutting down VM, before exiting");
            match vm.state() {
                VirtualMachineState::Stopped => break,
                VirtualMachineState::Stopping => {
                    println!("VM is stopping");
                    continue;
                }
                _ => continue,
            }
        }
    }

    pub fn setup_window(&self) {
        unsafe {
            let vm = self.ivars().vm.clone();
            let mtm = self.ivars().mtm;
            let view = VZVirtualMachineView::new(mtm);
            view.setCapturesSystemKeys(true);
            view.setVirtualMachine(Some(vm.machine.as_ref()));
            //macos 14 and up
            view.setAutomaticallyReconfiguresDisplay(true);

            let origin = NSPoint::new(10.0, 10.0);
            let size = NSSize::new(1024.0, 768.0);
            let rect = NSRect::new(origin, size);
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            );

            window.setOpaque(false);
            let object = ProtocolObject::from_ref(self);
            window.setDelegate(Some(object));
            window.setContentView(Some(view.as_super()));
            window.setInitialFirstResponder(Some(view.as_super()));
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            window.center();

            // create menu
            window.makeKeyAndOrderFront(Some(view.as_super()));
            window.setReleasedWhenClosed(false);

            if !NSRunningApplication::currentApplication().isActive() {
                NSRunningApplication::currentApplication()
                    .activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
            }
        }
    }
}
