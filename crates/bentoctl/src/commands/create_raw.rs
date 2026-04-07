use bento_core::Mount;
use bento_libvm::{CreateRawMachineRequest, LibVm};
use bento_runtime::instance::{MountConfig, NetworkMode};
use clap::Args;
use eyre::Context;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,
    #[arg(long, default_value_t = 1, help = "number of virtual CPUs")]
    pub cpus: u8,
    #[arg(
        long,
        default_value_t = 512,
        help = "virtual machine RAM size in mibibytes"
    )]
    pub memory: u32,
    #[arg(long, help = "Path to a custom kernel, only works for Linux.")]
    pub kernel: Option<PathBuf>,
    #[arg(
        long = "initramfs",
        visible_alias = "initrd",
        help = "Path to a custom initramfs image, only works for Linux."
    )]
    pub initramfs: Option<PathBuf>,
    #[arg(long, value_name = "PATH", help = "Path to an existing rootfs image")]
    pub rootfs: Option<PathBuf>,
    #[arg(
        long,
        value_name = "GB",
        help = "Create an empty sparse rootfs image of this size in GB"
    )]
    pub empty_rootfs: Option<u64>,
    #[arg(long, help = "Enable nested virtualization for supported VZ guests")]
    pub nested_virtualization: bool,
    #[arg(
        long,
        help = "Enable Rosetta for x86_64 Linux binaries in supported VZ guests"
    )]
    pub rosetta: bool,
    #[arg(
        long = "disk",
        value_name = "PATH",
        help = "Path to an existing data disk image"
    )]
    pub disks: Vec<PathBuf>,
    #[arg(long = "mount", value_name = "PATH:ro|rw", value_parser = crate::commands::create::parse_mount_arg)]
    pub mounts: Vec<MountConfig>,
    #[arg(long, value_name = "MODE", value_parser = crate::commands::create::parse_network_mode)]
    pub network: Option<NetworkMode>,
    #[arg(long = "profile", value_name = "PROFILE")]
    pub profiles: Vec<String>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let request = CreateRawMachineRequest {
            name: self.name.clone(),
            cpus: self.cpus,
            memory_mib: self.memory,
            kernel: resolve_optional_path(self.kernel.as_deref(), "kernel")?,
            initramfs: resolve_optional_path(self.initramfs.as_deref(), "initramfs")?,
            rootfs: resolve_optional_path(self.rootfs.as_deref(), "rootfs")?,
            empty_rootfs_gb: self.empty_rootfs,
            nested_virtualization: self.nested_virtualization,
            rosetta: self.rosetta,
            disks: resolve_existing_paths(&self.disks, "disk")?,
            mounts: self.mounts.iter().cloned().map(mount_to_spec).collect(),
            network: self.network.map(map_network_mode),
            profiles: self.profiles.clone(),
        };

        libvm.create_raw(request)?;

        println!("created {}", self.name);
        Ok(())
    }
}

fn resolve_optional_path(path: Option<&Path>, kind: &str) -> eyre::Result<Option<PathBuf>> {
    let Some(path) = path else {
        return Ok(None);
    };

    Ok(Some(resolve_existing_path(path, kind)?))
}

fn resolve_existing_paths(paths: &[PathBuf], kind: &str) -> eyre::Result<Vec<PathBuf>> {
    paths
        .iter()
        .map(|path| resolve_existing_path(path, kind))
        .collect()
}

fn resolve_existing_path(path: &Path, kind: &str) -> eyre::Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let abs = std::fs::canonicalize(&abs)
        .context(format!("{kind} path does not exist: {}", abs.display()))?;

    Ok(abs)
}

fn map_network_mode(mode: NetworkMode) -> bento_core::NetworkMode {
    match mode {
        NetworkMode::VzNat => bento_core::NetworkMode::User,
        NetworkMode::None => bento_core::NetworkMode::None,
        NetworkMode::Bridged => bento_core::NetworkMode::Bridged,
        NetworkMode::Cni => bento_core::NetworkMode::User,
    }
}

fn mount_to_spec(mount: MountConfig) -> Mount {
    Mount {
        source: mount.location,
        tag: String::new(),
        read_only: !mount.writable,
    }
}
