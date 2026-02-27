use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bento_runtime::images::capabilities::GuestCapabilities;
use bento_runtime::images::store::ImageStore;
use bento_runtime::instance::{
    InstanceFile, InstanceStatus, MountConfig, NetworkConfig, NetworkMode,
};
use bento_runtime::instance_manager::{
    InstanceCreateOptions, InstanceError, InstanceManager, NixDaemon,
};
use clap::{Args, ValueEnum};
use eyre::{bail, Context};

use crate::commands::shell;
use crate::terminal;

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
pub enum AttachMode {
    Ssh,
    Serial,
}

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,

    #[arg(long, value_enum, default_value_t = AttachMode::Ssh)]
    pub attach: AttachMode,

    #[arg(long, short = 'u')]
    pub user: Option<String>,

    #[arg(long)]
    pub keep: bool,

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

    #[arg(long, help = "Base image name or OCI reference")]
    pub image: Option<String>,

    #[arg(
        long = "disk",
        value_name = "PATH",
        help = "Path to an existing disk image"
    )]
    pub disks: Vec<PathBuf>,

    #[arg(long = "mount", value_name = "PATH:ro|rw", value_parser = parse_mount_arg)]
    pub mounts: Vec<MountConfig>,

    #[arg(long, value_name = "PATH", help = "Path to userdata file")]
    pub userdata: Option<PathBuf>,

    #[arg(long, value_name = "MODE", value_parser = parse_network_mode)]
    pub network: Option<NetworkMode>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "run")
    }
}

impl Cmd {
    pub fn run(&self) -> eyre::Result<()> {
        let name = self.name.clone();

        let exe = std::env::current_exe().context("resolve bentoctl binary path")?;
        let daemon = NixDaemon::new(exe)
            .arg("instanced")
            .arg("--name")
            .arg(&self.name);
        let mut manager = InstanceManager::new(daemon);
        let mut store = ImageStore::open()?;

        let kernel_path = resolve_optional_path(self.kernel.as_deref(), "kernel")?;
        let initramfs_path = resolve_optional_path(self.initramfs.as_deref(), "initramfs")?;
        let userdata_path = resolve_optional_path(self.userdata.as_deref(), "userdata")?;
        let disk_paths = resolve_existing_paths(&self.disks, "disk")?;

        let selected_image = self
            .image
            .as_deref()
            .map(|image_arg| -> eyre::Result<_> {
                Ok(match store.resolve(image_arg)? {
                    Some(image) => image,
                    None => store.pull(image_arg, None)?,
                })
            })
            .transpose()?;

        let capabilities = selected_image
            .as_ref()
            .map(|image| GuestCapabilities::from_annotations(&image.annotations))
            .unwrap_or_default();

        let options = InstanceCreateOptions::default()
            .with_cpus(self.cpus)
            .with_memory(self.memory)
            .with_kernel(kernel_path)
            .with_initramfs(initramfs_path)
            .with_disks(disk_paths)
            .with_network(self.network.map(|mode| NetworkConfig { mode }))
            .with_capabilities(capabilities)
            .with_userdata(userdata_path);

        let mut created = false;
        let mut started = false;

        let run_result = (|| -> eyre::Result<()> {
            let inst = manager.create(&name, options)?;
            created = true;

            if let Some(image) = &selected_image {
                store.clone_base_image(image, &inst.file(InstanceFile::RootDisk))?;
            }

            manager.start(&inst)?;

            started = true;

            match self.attach {
                AttachMode::Ssh => attach_ssh(&name, self.user.as_deref()),
                AttachMode::Serial => terminal::attach_serial(&name),
            }
        })();

        let cleanup_result = if self.keep {
            Ok(())
        } else {
            cleanup_run_instance(&manager, &name, created, started)
        };

        if let Err(run_err) = run_result {
            if let Err(cleanup_err) = cleanup_result {
                return Err(run_err).context(format!("cleanup failed: {cleanup_err}"));
            }
            return Err(run_err);
        }

        cleanup_result
    }
}

fn cleanup_run_instance(
    manager: &InstanceManager<NixDaemon>,
    name: &str,
    created: bool,
    started: bool,
) -> eyre::Result<()> {
    if !created {
        return Ok(());
    }

    let inst = manager.inspect(name)?;

    if started && inst.status() == InstanceStatus::Running {
        match manager.stop(&inst) {
            Ok(()) => {}
            Err(InstanceError::InstanceNotRunning { .. }) => {}
            Err(err) => return Err(err.into()),
        }
    }

    std::thread::sleep(Duration::from_millis(200));

    let inst = manager.inspect(name)?;
    manager.delete(&inst)?;
    Ok(())
}

fn attach_ssh(name: &str, user: Option<&str>) -> eyre::Result<()> {
    let mut command = shell::build_ssh_command(name, user)?;
    let status = command.status().context("run ssh client")?;
    if status.success() {
        return Ok(());
    }

    match status.code() {
        Some(code) => bail!("ssh exited with status code {code}"),
        None => bail!("ssh terminated by signal"),
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

fn parse_mount_arg(input: &str) -> Result<MountConfig, String> {
    let (location, mode) = input
        .rsplit_once(':')
        .ok_or_else(|| "invalid mount, expected PATH:ro|rw".to_string())?;

    if location.is_empty() {
        return Err("invalid mount, path cannot be empty".to_string());
    }

    let writable = match mode {
        "rw" => true,
        "ro" => false,
        _ => {
            return Err(format!(
                "invalid mount mode '{mode}', expected 'ro' or 'rw'"
            ))
        }
    };

    Ok(MountConfig {
        location: PathBuf::from(location),
        writable,
    })
}

fn parse_network_mode(input: &str) -> Result<NetworkMode, String> {
    match input {
        "vznat" => Ok(NetworkMode::VzNat),
        "none" => Ok(NetworkMode::None),
        "bridged" => Ok(NetworkMode::Bridged),
        "cni" => Ok(NetworkMode::Cni),
        _ => Err(format!(
            "invalid network mode '{input}', expected one of: vznat, none, bridged, cni"
        )),
    }
}
