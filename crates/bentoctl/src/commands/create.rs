use bento_runtime::image_store::ImageStore;
use bento_runtime::instance::{InstanceFile, MountConfig, NetworkConfig, NetworkMode};
use bento_runtime::instance_manager::{InstanceCreateOptions, InstanceManager, NixDaemon};
use clap::Args;
use eyre::Context;
use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
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
    #[arg(long, help = "Base image name or OCI reference")]
    pub image: Option<String>,
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

        let kernel_path = match &self.kernel {
            Some(path) => {
                let abs = if path.is_absolute() {
                    path.clone()
                } else {
                    std::env::current_dir()?.join(path)
                };

                let abs = std::fs::canonicalize(&abs)
                    .context(format!("kernel path does not exist: {}", abs.display()))?;
                Some(abs)
            }
            None => None,
        };

        let options = InstanceCreateOptions::default()
            .with_cpus(self.cpus)
            .with_memory(self.memory)
            .with_kernel(kernel_path)
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
}
