use bento_runtime::image_store::ImageStore;
use bento_runtime::instance::{InstanceFile, MountConfig, NetworkConfig, NetworkMode};
use bento_runtime::instance_manager::{InstanceCreateOptions, InstanceManager, NixDaemon};
use clap::Args;
use eyre::Context;
use std::{
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

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
    #[arg(long, value_name = "MODE", value_parser = parse_network_mode)]
    pub network: Option<NetworkMode>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Cmd {
    pub fn run(&self) -> eyre::Result<()> {
        let daemon = NixDaemon::new("123");
        let manager = InstanceManager::new(daemon);
        let mut store = ImageStore::open()?;

        let kernel_path = resolve_optional_path(self.kernel.as_deref(), "kernel")?;
        let initramfs_path = resolve_optional_path(self.initramfs.as_deref(), "initramfs")?;
        let disk_paths = resolve_existing_paths(&self.disks, "disk")?;

        let options = InstanceCreateOptions::default()
            .with_cpus(self.cpus)
            .with_memory(self.memory)
            .with_kernel(kernel_path)
            .with_initramfs(initramfs_path)
            .with_disks(disk_paths)
            .with_mounts(self.mounts.clone())
            .with_network(self.network.map(|mode| NetworkConfig { mode }));

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

        let inst = manager.create(&self.name, options)?;

        if let Some(image) = selected_image {
            store.clone_base_image(&image, &inst.file(InstanceFile::RootDisk))?;
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_mount_arg_accepts_ro_and_rw() {
        let ro = parse_mount_arg("~/code:ro").expect("ro mount should parse");
        assert_eq!(ro.location, PathBuf::from("~/code"));
        assert!(!ro.writable);

        let rw = parse_mount_arg("/tmp/data:rw").expect("rw mount should parse");
        assert_eq!(rw.location, PathBuf::from("/tmp/data"));
        assert!(rw.writable);
    }

    #[test]
    fn parse_mount_arg_rejects_invalid_input() {
        assert!(parse_mount_arg("/tmp/data").is_err());
        assert!(parse_mount_arg(":rw").is_err());
        assert!(parse_mount_arg("/tmp/data:readwrite").is_err());
    }

    #[test]
    fn parse_network_mode_accepts_expected_values() {
        assert_eq!(
            parse_network_mode("vznat").expect("vznat should parse"),
            NetworkMode::VzNat
        );
        assert_eq!(
            parse_network_mode("none").expect("none should parse"),
            NetworkMode::None
        );
        assert_eq!(
            parse_network_mode("bridged").expect("bridged should parse"),
            NetworkMode::Bridged
        );
        assert_eq!(
            parse_network_mode("cni").expect("cni should parse"),
            NetworkMode::Cni
        );
    }

    #[test]
    fn parse_network_mode_rejects_invalid_value() {
        assert!(parse_network_mode("default").is_err());
    }

    #[test]
    fn resolve_optional_path_returns_none_when_unset() {
        let resolved = resolve_optional_path(None, "kernel").expect("unset path should resolve");
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_optional_path_canonicalizes_relative_paths() {
        let old_cwd = std::env::current_dir().expect("cwd should resolve");
        let tmp = TempDir::new().expect("temp dir should be creatable");
        let nested = tmp.path().join("boot");
        std::fs::create_dir_all(&nested).expect("nested dir should be creatable");
        let file = nested.join("initramfs.img");
        std::fs::write(&file, b"initramfs").expect("initramfs file should be creatable");

        std::env::set_current_dir(tmp.path()).expect("set cwd should succeed");
        let resolved = resolve_optional_path(Some(Path::new("boot/./initramfs.img")), "initramfs")
            .expect("relative path should resolve")
            .expect("path should be present");
        std::env::set_current_dir(old_cwd).expect("restore cwd should succeed");

        let canonical_file =
            std::fs::canonicalize(file).expect("test initramfs file should canonicalize");
        assert_eq!(resolved, canonical_file);
    }

    #[test]
    fn resolve_existing_paths_rejects_missing_path() {
        let err = resolve_existing_paths(&[PathBuf::from("/definitely/not/here/disk.img")], "disk")
            .expect_err("missing disk should fail");

        assert!(err.to_string().contains("disk path does not exist"));
    }
}
