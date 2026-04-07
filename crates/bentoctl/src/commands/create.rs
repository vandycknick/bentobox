use bento_core::Mount;
use bento_libvm::{CreateMachineRequest, LibVm};
use bento_runtime::instance::{MountConfig, NetworkMode};
use clap::Args;
use eyre::Context;
use std::{
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

#[derive(Args, Debug)]
pub struct Cmd {
    pub image_ref: String,
    pub name: String,
    #[arg(long, help = "number of virtual CPUs")]
    pub cpus: Option<u8>,
    #[arg(long, help = "virtual machine RAM size in mibibytes")]
    pub memory: Option<u32>,
    #[arg(long, help = "Path to a custom kernel, only works for Linux.")]
    pub kernel: Option<PathBuf>,
    #[arg(
        long = "initramfs",
        visible_alias = "initrd",
        help = "Path to a custom initramfs image, only works for Linux."
    )]
    pub initramfs: Option<PathBuf>,
    #[arg(
        long,
        value_name = "GB",
        help = "Resize the image-backed root disk to this size in GB"
    )]
    pub disk_size: Option<u64>,
    #[arg(long, help = "Enable nested virtualization for supported VZ guests")]
    pub nested_virtualization: bool,
    #[arg(
        long,
        help = "Enable Rosetta for x86_64 Linux binaries in supported VZ guests"
    )]
    pub rosetta: bool,
    #[arg(long, value_name = "PATH", help = "Path to userdata file")]
    pub userdata: Option<PathBuf>,
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
        let request = CreateMachineRequest {
            image_ref: self.image_ref.clone(),
            name: self.name.clone(),
            cpus: self.cpus,
            memory_mib: self.memory,
            kernel: resolve_optional_path(self.kernel.as_deref(), "kernel")?,
            initramfs: resolve_optional_path(self.initramfs.as_deref(), "initramfs")?,
            disk_size_gb: self.disk_size,
            nested_virtualization: self.nested_virtualization,
            rosetta: self.rosetta,
            userdata: resolve_optional_path(self.userdata.as_deref(), "userdata")?,
            disks: resolve_existing_paths(&self.disks, "disk")?,
            mounts: self.mounts.iter().cloned().map(mount_to_spec).collect(),
            network: self.network.map(map_network_mode),
            profiles: self.profiles.clone(),
        };

        libvm.create_from_image(request)?;

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

pub(crate) fn parse_mount_arg(input: &str) -> Result<MountConfig, String> {
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

pub(crate) fn parse_network_mode(input: &str) -> Result<NetworkMode, String> {
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

#[cfg(test)]
fn gigabytes_to_bytes(size_gb: u64) -> u64 {
    size_gb.saturating_mul(1_000_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{BentoCtlCmd, Command};
    use clap::Parser;
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
    fn create_command_parses_nested_virtualization_flag() {
        let cmd = BentoCtlCmd::try_parse_from([
            "bentoctl",
            "create",
            "ghcr.io/acme/base:latest",
            "dev",
            "--nested-virtualization",
        ])
        .expect("create command should parse");

        let create = match cmd.cmd {
            Command::Create(cmd) => cmd,
            other => panic!("expected create command, got {other:?}"),
        };

        assert!(create.nested_virtualization);
    }

    #[test]
    fn create_command_parses_rosetta_flag() {
        let cmd = BentoCtlCmd::try_parse_from([
            "bentoctl",
            "create",
            "ghcr.io/acme/base:latest",
            "dev",
            "--rosetta",
        ])
        .expect("create command should parse");

        let create = match cmd.cmd {
            Command::Create(cmd) => cmd,
            other => panic!("expected create command, got {other:?}"),
        };

        assert!(create.rosetta);
    }

    #[test]
    fn create_command_parses_disk_size_flag() {
        let cmd = BentoCtlCmd::try_parse_from([
            "bentoctl",
            "create",
            "example.com/acme/image:latest",
            "dev",
            "--disk-size",
            "40",
        ])
        .expect("create command should parse");

        let create = match cmd.cmd {
            Command::Create(cmd) => cmd,
            other => panic!("expected create command, got {other:?}"),
        };

        assert_eq!(create.disk_size, Some(40));
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

    #[test]
    fn gigabytes_to_bytes_uses_decimal_units() {
        assert_eq!(gigabytes_to_bytes(40), 40_000_000_000);
    }
}
