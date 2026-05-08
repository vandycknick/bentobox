use std::path::{Path, PathBuf};

use bento_krun::{validate_config, KrunConfig};
use bento_krun_sys::{ctx, DiskFormat, Feature, KernelFormat, SyncMode};
use clap::Parser;

#[path = "../internal/parse.rs"]
mod parse;

#[derive(Debug, Parser)]
#[command(name = "krun", about = "BentoBox libkrun helper")]
struct Cli {
    #[arg(long, default_value_t = 1)]
    cpus: u8,
    #[arg(long, default_value_t = 512)]
    memory_mib: u32,
    #[arg(long)]
    kernel: Option<PathBuf>,
    #[arg(long)]
    initramfs: Option<PathBuf>,
    #[arg(long = "cmdline")]
    cmdline: Vec<String>,
    #[arg(long = "disk", value_parser = parse::disk)]
    disks: Vec<bento_krun::Disk>,
    #[arg(long = "mount", value_parser = parse::mount)]
    mounts: Vec<bento_krun::Mount>,
    #[arg(long = "vsock-port", value_parser = parse::vsock_port)]
    vsock_ports: Vec<bento_krun::VsockPort>,
    #[arg(long = "net-unixgram", value_parser = parse::net_unixgram)]
    net_unixgrams: Vec<bento_krun::NetUnixgram>,
    #[arg(long)]
    stdio_console: bool,
    #[arg(long)]
    disable_implicit_vsock: bool,
}

impl Cli {
    fn into_config(self) -> KrunConfig {
        KrunConfig {
            cpus: self.cpus,
            memory_mib: self.memory_mib,
            kernel: self.kernel,
            initramfs: self.initramfs,
            cmdline: self.cmdline,
            disks: self.disks,
            mounts: self.mounts,
            vsock_ports: self.vsock_ports,
            net_unixgrams: self.net_unixgrams,
            stdio_console: self.stdio_console,
            disable_implicit_vsock: self.disable_implicit_vsock,
        }
    }
}

fn main() -> eyre::Result<()> {
    let cli = Cli::parse();
    let config = cli.into_config();
    validate_config(&config)?;
    start_enter(&config)?;
    Ok(())
}

fn start_enter(config: &KrunConfig) -> eyre::Result<()> {
    let ctx_id = ctx::create_ctx()?;
    let configured = configure_ctx(ctx_id, config);
    if let Err(err) = configured {
        let _ = ctx::free_ctx(ctx_id);
        return Err(err);
    }
    ctx::start_enter(ctx_id)?;
    Ok(())
}

fn configure_ctx(ctx_id: u32, config: &KrunConfig) -> eyre::Result<()> {
    ctx::set_vm_config(ctx_id, config.cpus, config.memory_mib)?;

    if let Some(kernel) = config.kernel.as_ref() {
        let cmdline = (!config.cmdline.is_empty()).then(|| config.cmdline.join(" "));
        ctx::set_kernel(
            ctx_id,
            &path_string(kernel),
            KernelFormat::Raw,
            config
                .initramfs
                .as_ref()
                .map(|path| path_string(path))
                .as_deref(),
            cmdline.as_deref(),
        )?;
    }

    for disk in &config.disks {
        require_feature(Feature::Blk, "block devices (--disk)")?;
        ctx::add_disk3(
            ctx_id,
            &disk.block_id,
            &path_string(&disk.path),
            DiskFormat::Raw,
            disk.read_only,
            false,
            SyncMode::Relaxed,
        )?;
    }

    for mount in &config.mounts {
        ctx::add_virtiofs3(
            ctx_id,
            &mount.tag,
            &path_string(&mount.path),
            0,
            mount.read_only,
        )?;
    }

    for port in &config.vsock_ports {
        ctx::add_vsock_port2(ctx_id, port.port, &path_string(&port.path), port.listen)?;
    }

    for net in &config.net_unixgrams {
        require_feature(Feature::Net, "userspace networking (--net-unixgram)")?;
        ctx::add_net_unixgram(ctx_id, &path_string(&net.path), net.mac)?;
    }

    if config.stdio_console {
        ctx::disable_implicit_console(ctx_id)?;
        ctx::add_virtio_console_default(ctx_id, 0, 1, 2)?;
        ctx::set_kernel_console(ctx_id, "hvc0")?;
    }

    if config.disable_implicit_vsock {
        ctx::disable_implicit_vsock(ctx_id)?;
    }

    Ok(())
}

fn require_feature(feature: Feature, requested_by: &'static str) -> eyre::Result<()> {
    if ctx::has_feature(feature)? {
        return Ok(());
    }

    eyre::bail!(
        "unsupported libkrun feature: {requested_by} requires libkrun feature {}; rebuild or install a libkrun with {} support",
        feature_name(feature),
        feature_name(feature)
    )
}

fn feature_name(feature: Feature) -> &'static str {
    match feature {
        Feature::Net => "NET",
        Feature::Blk => "BLK",
        Feature::Gpu => "GPU",
        Feature::Snd => "SND",
        Feature::Input => "INPUT",
        Feature::Efi => "EFI",
        Feature::Tee => "TEE",
        Feature::AmdSev => "AMD_SEV",
        Feature::IntelTdx => "INTEL_TDX",
        Feature::AwsNitro => "AWS_NITRO",
        Feature::VirglResourceMap2 => "VIRGL_RESOURCE_MAP2",
    }
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}
