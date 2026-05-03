use std::path::PathBuf;

use bento_krun::{Disk, Mount, VirtualMachineBuilder, VsockPort};
use bento_krun_sys::{ctx, Feature};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "krun", about = "BentoBox libkrun helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run(Box<Run>),
    CheckNestedVirt,
    MaxVcpus,
    HasFeature { feature: FeatureArg },
}

#[derive(Debug, Parser)]
struct Run {
    #[arg(long, default_value_t = 1)]
    cpus: u8,
    #[arg(long, default_value_t = 512)]
    memory_mib: u32,
    #[arg(long)]
    kernel: Option<PathBuf>,
    #[arg(long)]
    initramfs: Option<PathBuf>,
    #[arg(long = "cmdline")]
    kernel_cmdline: Vec<String>,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long = "disk", value_parser = parse_disk)]
    disks: Vec<Disk>,
    #[arg(long = "mount", value_parser = parse_mount)]
    mounts: Vec<Mount>,
    #[arg(long = "vsock-port", value_parser = parse_vsock_port)]
    vsock_ports: Vec<VsockPort>,
    #[arg(long)]
    console_output: Option<PathBuf>,
    #[arg(long)]
    disable_implicit_vsock: bool,
    #[arg(long)]
    root_disk_remount_device: Option<String>,
    #[arg(long)]
    root_disk_remount_fstype: Option<String>,
    #[arg(long)]
    root_disk_remount_options: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum FeatureArg {
    Net,
    Blk,
    Gpu,
    Snd,
    Input,
    Tee,
    AmdSev,
    IntelTdx,
    AwsNitro,
    VirglResourceMap2,
}

fn main() -> eyre::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(run) => run_command(*run)?,
        Command::CheckNestedVirt => {
            println!("{}", ctx::check_nested_virt()?);
        }
        Command::MaxVcpus => {
            println!("{}", ctx::get_max_vcpus()?);
        }
        Command::HasFeature { feature } => {
            println!("{}", ctx::has_feature(map_feature(feature))?);
        }
    }
    Ok(())
}

fn run_command(run: Run) -> eyre::Result<()> {
    let mut builder = VirtualMachineBuilder::new()
        .cpus(run.cpus)
        .memory_mib(run.memory_mib)
        .kernel_cmdline(run.kernel_cmdline)
        .disable_implicit_vsock(run.disable_implicit_vsock);

    if let Some(kernel) = run.kernel {
        builder = builder.kernel(kernel);
    }
    if let Some(initramfs) = run.initramfs {
        builder = builder.initramfs(initramfs);
    }
    if let Some(root) = run.root {
        builder = builder.root(root);
    }
    if let Some(console_output) = run.console_output {
        builder = builder.console_output(console_output);
    }
    if let Some(device) = run.root_disk_remount_device {
        builder = builder.root_disk_remount(
            device,
            run.root_disk_remount_fstype,
            run.root_disk_remount_options,
        );
    }
    for disk in run.disks {
        builder = builder.disk(disk);
    }
    for mount in run.mounts {
        builder = builder.mount(mount);
    }
    for port in run.vsock_ports {
        builder = builder.vsock_port(port);
    }

    builder.start_enter()?;
    Ok(())
}

fn parse_disk(input: &str) -> Result<Disk, String> {
    let parts: Vec<&str> = input.split(':').collect();
    if parts.len() != 3 {
        return Err("expected BLOCK_ID:PATH:ro|rw".to_string());
    }
    Ok(Disk {
        block_id: parts[0].to_string(),
        path: PathBuf::from(parts[1]),
        read_only: parse_ro(parts[2])?,
    })
}

fn parse_mount(input: &str) -> Result<Mount, String> {
    let parts: Vec<&str> = input.split(':').collect();
    if parts.len() != 3 {
        return Err("expected TAG:PATH:ro|rw".to_string());
    }
    Ok(Mount {
        tag: parts[0].to_string(),
        path: PathBuf::from(parts[1]),
        read_only: parse_ro(parts[2])?,
    })
}

fn parse_vsock_port(input: &str) -> Result<VsockPort, String> {
    let parts: Vec<&str> = input.split(':').collect();
    if parts.len() != 3 {
        return Err("expected PORT:PATH:connect|listen".to_string());
    }
    let port = parts[0]
        .parse::<u32>()
        .map_err(|err| format!("invalid port: {err}"))?;
    let listen = match parts[2] {
        "listen" => true,
        "connect" => false,
        other => return Err(format!("invalid vsock direction {other:?}")),
    };
    Ok(VsockPort {
        port,
        path: PathBuf::from(parts[1]),
        listen,
    })
}

fn parse_ro(input: &str) -> Result<bool, String> {
    match input {
        "ro" => Ok(true),
        "rw" => Ok(false),
        other => Err(format!("invalid mode {other:?}, expected ro or rw")),
    }
}

fn map_feature(feature: FeatureArg) -> Feature {
    match feature {
        FeatureArg::Net => Feature::Net,
        FeatureArg::Blk => Feature::Blk,
        FeatureArg::Gpu => Feature::Gpu,
        FeatureArg::Snd => Feature::Snd,
        FeatureArg::Input => Feature::Input,
        FeatureArg::Tee => Feature::Tee,
        FeatureArg::AmdSev => Feature::AmdSev,
        FeatureArg::IntelTdx => Feature::IntelTdx,
        FeatureArg::AwsNitro => Feature::AwsNitro,
        FeatureArg::VirglResourceMap2 => Feature::VirglResourceMap2,
    }
}
