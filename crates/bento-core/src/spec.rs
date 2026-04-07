use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmSpec {
    pub version: u32,
    pub name: String,
    pub platform: Platform,
    pub resources: Resources,
    pub boot: Boot,
    pub storage: Storage,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    pub network: Network,
    pub guest: Guest,
    pub host: Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub guest_os: GuestOs,
    pub architecture: Architecture,
    pub backend: Backend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    pub cpus: u8,
    pub memory_mib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Boot {
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    #[serde(default)]
    pub kernel_cmdline: Vec<String>,
    pub bootstrap: Option<Bootstrap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bootstrap {
    pub cloud_init: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    #[serde(default)]
    pub disks: Vec<Disk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    pub path: PathBuf,
    pub kind: DiskKind,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskKind {
    Root,
    Data,
    Seed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    pub source: PathBuf,
    pub tag: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub mode: NetworkMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guest {
    #[serde(default)]
    pub profiles: Vec<String>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub nested_virtualization: bool,
    pub rosetta: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOs {
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    Aarch64,
    X86_64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Auto,
    Vz,
    Firecracker,
    CloudHypervisor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    None,
    User,
    Bridged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub ssh: bool,
    pub docker: bool,
    pub dns: bool,
    pub forward: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        Architecture, Backend, Boot, Bootstrap, Capabilities, Disk, DiskKind, Guest, GuestOs, Host,
        Mount, Network, NetworkMode, Platform, Resources, Storage, VmSpec,
    };
    use std::path::PathBuf;

    fn sample_vm_spec() -> VmSpec {
        VmSpec {
            version: 1,
            name: "dev".to_string(),
            platform: Platform {
                guest_os: GuestOs::Linux,
                architecture: Architecture::Aarch64,
                backend: Backend::Auto,
            },
            resources: Resources {
                cpus: 4,
                memory_mib: 4096,
            },
            boot: Boot {
                kernel: Some(PathBuf::from("/kernel")),
                initramfs: Some(PathBuf::from("/initramfs")),
                kernel_cmdline: vec!["console=hvc0".to_string(), "panic=-1".to_string()],
                bootstrap: Some(Bootstrap {
                    cloud_init: Some(PathBuf::from("/cloud-init/user-data")),
                }),
            },
            storage: Storage {
                disks: vec![
                    Disk {
                        path: PathBuf::from("/root.img"),
                        kind: DiskKind::Root,
                        read_only: false,
                    },
                    Disk {
                        path: PathBuf::from("/seed.img"),
                        kind: DiskKind::Seed,
                        read_only: true,
                    },
                ],
            },
            mounts: vec![Mount {
                source: PathBuf::from("/Users/nickvd/Projects/bentobox"),
                tag: "workspace".to_string(),
                read_only: false,
            }],
            network: Network {
                mode: NetworkMode::User,
            },
            guest: Guest {
                profiles: vec!["default".to_string(), "docker".to_string()],
                capabilities: Capabilities {
                    ssh: true,
                    docker: true,
                    dns: true,
                    forward: true,
                },
            },
            host: Host {
                nested_virtualization: false,
                rosetta: true,
            },
        }
    }

    #[test]
    fn vm_spec_round_trips_through_yaml() {
        let spec = sample_vm_spec();
        let yaml = serde_yaml_ng::to_string(&spec).expect("serialize vm spec");
        let decoded: VmSpec = serde_yaml_ng::from_str(&yaml).expect("deserialize vm spec");

        assert_eq!(decoded, spec);
    }

    #[test]
    fn vm_spec_yaml_uses_snake_case_enums() {
        let yaml = serde_yaml_ng::to_string(&sample_vm_spec()).expect("serialize vm spec");

        assert!(yaml.contains("guest_os: linux"));
        assert!(yaml.contains("architecture: aarch64"));
        assert!(yaml.contains("backend: auto"));
        assert!(yaml.contains("kind: root"));
        assert!(yaml.contains("kind: seed"));
        assert!(yaml.contains("mode: user"));
    }
}
