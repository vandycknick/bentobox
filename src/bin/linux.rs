use std::io::{stdin, stdout};

use bentobox::{
    termios::{get_terminal_attr, set_raw_mode, set_terminal_attr},
    vm::{VirtualMachine, VirtualMachineBuilder, VirtualMachineGuestPlatform, VirtualMachineState},
};
use crossbeam::{
    channel::{bounded, Receiver},
    select,
};

fn ctrl_channel() -> eyre::Result<Receiver<()>> {
    let (sender, receiver) = bounded(100);
    ctrlc::set_handler(move || {
        let _ = sender.send(());
    })?;

    Ok(receiver)
}

fn main() -> eyre::Result<()> {
    if VirtualMachine::supported() != true {
        return Err(eyre::eyre!(
            "Virtualization.Framework is not supported on this machine."
        ));
    }
    // let result = canonicalize("hello").context("failed normalizing the kernel path")?;

    let kernel = "/Users/nickvd/Projects/bentobox/target/boxos/arch/arm64/boot/Image";
    let initramfs = "/Users/nickvd/Projects/bentobox/target/boxos/initramfs";
    let disk = "/Users/nickvd/Projects/bentobox/target/boxos/ubuntu-22.04.img";

    let std_in = stdin();
    let std_out = stdout();

    let vm = VirtualMachineBuilder::new()
        .use_platform(VirtualMachineGuestPlatform::Linux {
            kernel: kernel.to_string(),
            initramfs: initramfs.to_string(),
            command_line: None,
        })
        .use_console(Some(&std_in), Some(&std_out))
        .use_memory_balloon()
        .use_entropy_device()
        .use_network()
        .use_cpus(4)
        .use_memory(2147483648)
        .use_storage_device(&disk)
        .build();

    if !vm.can_start() {
        return Err(eyre::eyre!("Machine can't start"));
    }

    // TODO: fix the result type coming from vm.start
    vm.start().unwrap();

    let termios = get_terminal_attr(&std_in)?;
    set_raw_mode(&std_in)?;

    let state_changes = vm.get_state_channel();
    let ctrl_c_events = ctrl_channel()?;

    loop {
        select! {
            recv(state_changes) -> state => {
                match state {
                    Ok(VirtualMachineState::Running) => println!("Virtual machine is running!"),
                    Ok(VirtualMachineState::Stopped) => {
                        println!("Virtual machine has stopped, exiting!");
                        break;
                    }
                    Ok(d) => println!("Virtual machine is in state: {}.", d),
                    Err(_) => {},
                }
            }
            recv(ctrl_c_events) -> _ => {
                set_terminal_attr(&std_in, &termios).expect("Failed to reset tty back to original state!");

                if vm.can_stop() {
                    let _  = vm.stop();
                }

                break;
            }
        }
    }

    println!("\nExiting");
    Ok(())
}
