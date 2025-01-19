use core::fmt;
use std::{
    fs::{create_dir_all, File},
    path::Path,
};

use anyhow::anyhow;
use bentobox::queue::Queue;
use bentobox::vm::{VirtualMachine, VirtualMachineBuilder, VirtualMachineState};
use bentobox::{
    downloader::ipsw::IpswRegistry,
    internal::{_VZVNCAuthenticationSecurityConfiguration, _VZVNCServer},
};

use bentobox::fs::{get_app_support_dir, get_cache_dir, get_preferences_dir};
use bentobox::utils::size_in_bytes;
use clap::{Parser, Subcommand, ValueEnum};
use crossbeam::{
    channel::{bounded, Receiver},
    select,
};
use objc2::ClassType;
use objc2_foundation::{ns_string, NSString};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long, short)]
    debug: Option<bool>,
}

#[derive(Subcommand)]
enum Commands {
    Create {
        #[arg(value_name = "DISTRO:[VERSION]")]
        distro: String,

        name: Option<String>,

        #[arg(long, short, value_enum, default_value_t=Architecture::Current)]
        arch: Architecture,
    },
    Start {
        name: Option<String>,
    },
    // Stop {},
    Test,
}

#[derive(ValueEnum, Clone, Debug)]
enum Architecture {
    #[value(hide = true)]
    Current,
    Arm64,
    Amd64,
}

impl fmt::Display for Architecture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Architecture::Arm64 => write!(f, "arm64"),
            Architecture::Amd64 => write!(f, "amd64"),
            Architecture::Current => write!(f, "current"),
        }
    }
}

fn create_command(name: &str, distro: &str, arch: Architecture) -> anyhow::Result<()> {
    if VirtualMachine::supported() != true {
        return Err(anyhow!(
            "Virtualization.Framework is not supported on this machine."
        ));
    }

    let (distro_name, distro_version) = distro.split_once(":").unwrap_or((distro, ""));
    let name = if name == "" { distro_name } else { name };

    let app_dir = get_app_support_dir().unwrap_or("~/.local/share".to_string());
    let vm_path = Path::new(&app_dir).join("BentoBox").join(name);

    if vm_path.exists() {
        return Err(anyhow!("VM with name {} already exists!", name));
    }

    create_dir_all(&vm_path).unwrap();

    // This is mac specific
    let aux_path = vm_path.join("aux.img");
    let aux_file = File::create_new(&aux_path).unwrap();
    aux_file.set_len(size_in_bytes("30mb")).unwrap();

    let image_path = vm_path.join("disk.img");
    let image_file = File::create_new(&image_path).unwrap();
    image_file.set_len(size_in_bytes("50gb")).unwrap();

    let arch = match arch {
        Architecture::Current => Architecture::Arm64,
        _ => arch.clone(),
    };

    println!(
        "Creating a new vm: {} with distro {} and version {} for arch {}.",
        name, distro_name, distro_version, arch
    );

    let vm = VirtualMachineBuilder::new()
        .use_cpus(4)
        .use_memory(4294967296)
        .use_platform_macos(aux_path.to_str().unwrap(), None)
        .use_storage_device(image_path.to_str().unwrap())
        .use_network()
        .build();

    let registry = IpswRegistry::new();
    let file = registry.download(distro_version)?;

    // let cache_dir = get_cache_dir().unwrap();
    // let installer_path = Path::new(&cache_dir)
    //     .join("codes.nvd.BentoBox")
    //     .join("./UniversalMac_12.0.1_21A559_Restore.ipsw");

    vm.install_macos(file);

    Ok(())
}

fn ctrl_channel() -> Result<Receiver<()>, ctrlc::Error> {
    let (sender, receiver) = bounded(100);
    ctrlc::set_handler(move || {
        let _ = sender.send(());
    })?;

    Ok(receiver)
}

fn start_command(name: &str) -> Result<(), String> {
    println!("Starting machine: {}", name);
    if VirtualMachine::supported() != true {
        return Err("Virtualization.Framework is not supported on this machine.".to_string());
    }

    let app_dir = get_app_support_dir().unwrap_or("~/.local/share".to_string());
    let vm_path = Path::new(&app_dir).join("BentoBox").join(name);

    // This is mac specific
    let aux_path = vm_path.join("aux.img");
    let image_path = vm_path.join("disk.img");

    let vm = VirtualMachineBuilder::new()
        .use_cpus(4)
        .use_memory(4294967296)
        .use_platform_macos(
            aux_path.to_str().unwrap(),
            Some(
                "YnBsaXN0MDDRAQJURUNJRBN35Dzb6jwcQwgLEAAAAAAAAAEBAAAAAAAAAAMAAAAAAAAAAAAAAAAAAAAZ",
            ),
        )
        .use_storage_device(image_path.to_str().unwrap())
        .use_network()
        .use_keyboard()
        .use_graphics_device()
        .build();

    if !vm.can_start() {
        return Err("Machine can't be started".to_string());
    }

    vm.start()?;

    let sec_config = unsafe {
        _VZVNCAuthenticationSecurityConfiguration::initWithPassword(
            _VZVNCAuthenticationSecurityConfiguration::alloc(),
            ns_string!("password"),
        )
    };

    let vnc = unsafe {
        _VZVNCServer::initWithPort_queue_securityConfiguration(
            _VZVNCServer::alloc(),
            0,
            Queue::global().ptr,
            &sec_config,
        )
    };
    unsafe {
        vnc.setVirtualMachine(Some(&vm.machine));
        vnc.start();

        loop {
            let port = vnc.port();

            if port != 0 {
                println!("vnc://:password@127.0.0.1:{}", port);
                break;
            }
        }
    };

    let state_changes = vm.get_state_channel();
    let ctrl_c_events = ctrl_channel().unwrap();

    loop {
        println!("Start of the loop");
        select! {
            // Once ctrl-c is pressed the machine will stop synchronously. Hence this listener
            // isn't active anymore. This exists for when the VM gets shutdown from inside the
            // running OS.
            recv(state_changes) -> state => {
                println!("Am I even getting here");
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
                println!("Ctrlc is pressed, going to try and stop the vm");

                if vm.can_stop() {
                    vm.stop().expect("stopping vm should not fail at this point");
                    break;
                } else {
                    // NOTE: should probably figure out what to do here.
                    break;
                }
            }
        }
        println!("End of the loop");
    }

    // vm.open_window();

    // NOTE: This doesn't work. App run takes over the app and will exit once it closes. No code
    // will ever run after open_window
    // println!("Am I even getting here?");
    //
    // // NOTE: force stopping the VM for now.
    // vm.stop().unwrap();
    //
    // let mut cnt = 0;
    // loop {
    //     cnt += 1;
    //
    //     if cnt > 10 {
    //         println!("VM took longer than 10 seconds to stop, exiting now!");
    //         break;
    //     }
    //
    //     std::thread::sleep(std::time::Duration::from_secs(1));
    //     println!("Shutting down VM, before exiting");
    //     match vm.state() {
    //         VirtualMachineState::Stopped => break,
    //         VirtualMachineState::Stopping => {
    //             println!("VM is stopping");
    //             continue;
    //         }
    //         _ => continue,
    //     }
    // }

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Create { name, distro, arch }) => create_command(
            name.as_ref().and_then(|n| Some(n.as_str())).unwrap_or(""),
            distro.as_str(),
            arch.clone(),
        )
        .unwrap(),

        Some(Commands::Start { name }) => start_command(name.as_ref().unwrap()).unwrap(),
        Some(Commands::Test) => {
            println!("{:?}", get_app_support_dir());
            println!("{:?}", get_cache_dir());
            println!("{:?}", get_preferences_dir());
            let config = unsafe {
                _VZVNCAuthenticationSecurityConfiguration::initWithPassword(
                    _VZVNCAuthenticationSecurityConfiguration::alloc(),
                    &NSString::from_str("password"),
                )
            };
            println!("{:?}", unsafe { config.password() });
        }
        None => {}
    }
}
