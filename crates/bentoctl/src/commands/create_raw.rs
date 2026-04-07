use bento_runtime::capabilities::CapabilitiesConfig;
use bento_runtime::instance::{
    BootstrapConfig, InstanceFile, MountConfig, NetworkConfig, NetworkMode,
};
use bento_runtime::instance_store::{InstanceCreateOptions, InstanceStore};
use bento_vmmon::machine::prepare_instance;
use clap::Args;
use eyre::Context;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

const BYTES_PER_GB: u64 = 1_000_000_000;

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
    pub async fn run(&self, store: &InstanceStore) -> eyre::Result<()> {
        if self.rootfs.is_some() && self.empty_rootfs.is_some() {
            eyre::bail!("--rootfs and --empty-rootfs are mutually exclusive");
        }
        if matches!(self.empty_rootfs, Some(0)) {
            eyre::bail!("--empty-rootfs must be greater than 0");
        }

        let kernel_path = resolve_optional_path(self.kernel.as_deref(), "kernel")?;
        let initramfs_path = resolve_optional_path(self.initramfs.as_deref(), "initramfs")?;
        let rootfs_path = resolve_optional_path(self.rootfs.as_deref(), "rootfs")?;
        let disk_paths = resolve_existing_paths(&self.disks, "disk")?;

        let capabilities = CapabilitiesConfig::default();
        let bootstrap = (capabilities.requires_bootstrap() || self.rosetta)
            .then(BootstrapConfig::cidata_cloud_init);

        let options = InstanceCreateOptions::default()
            .with_cpus(self.cpus)
            .with_memory(self.memory)
            .with_kernel(kernel_path)
            .with_initramfs(initramfs_path)
            .with_root_disk(rootfs_path)
            .with_nested_virtualization(self.nested_virtualization)
            .with_rosetta(self.rosetta)
            .with_disks(disk_paths)
            .with_mounts(self.mounts.clone())
            .with_network(self.network.map(|mode| NetworkConfig { mode }))
            .with_bootstrap(bootstrap)
            .with_profiles(self.profiles.clone())
            .with_capabilities(capabilities);

        let pending = store.create_pending(&self.name, options)?;
        let inst = pending.instance();

        prepare_instance(inst)?;

        if let Some(size_gb) = self.empty_rootfs {
            let rootfs = inst.file(InstanceFile::RootDisk);
            std::fs::File::create(&rootfs)
                .context("create empty rootfs file")?
                .set_len(size_gb.saturating_mul(BYTES_PER_GB))
                .context("size empty rootfs file")?;
        }

        pending.commit()?;

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
