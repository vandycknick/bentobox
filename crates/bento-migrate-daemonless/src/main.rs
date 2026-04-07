use std::fs;
use std::num::NonZeroI32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bento_core::{
    Architecture, Backend, Boot, Bootstrap, Capabilities, Disk, DiskKind, Guest, GuestOs, Host,
    MachineId, Mount, Network, NetworkMode, Platform, Resources, Storage, VmSpec,
};
use bento_libvm::{Layout, LibVm};
use bento_runtime::instance::{
    default_engine_type, resolve_network_mode, DiskRole, EngineType, GuestOs as LegacyGuestOs,
    InstanceConfig, InstanceFile, NetworkMode as LegacyNetworkMode,
};
use clap::Parser;
use nix::sys::signal;
use nix::unistd::Pid;
use thiserror::Error;

#[derive(Parser, Debug)]
#[command(name = "bento-migrate-daemonless")]
struct Args {
    #[arg(long)]
    all: bool,

    #[arg(long)]
    execute: bool,

    #[arg(value_name = "NAME")]
    names: Vec<String>,
}

#[derive(Debug, Error)]
enum MigrationError {
    #[error("use --all or provide one or more VM names")]
    MissingSelection,

    #[error("cannot combine --all with explicit VM names")]
    MixedSelection,

    #[error("old-world VM {name:?} not found at {path}")]
    VmNotFound { name: String, path: PathBuf },

    #[error("refusing to migrate running VM {name:?}: live pid {pid} in {path}")]
    VmRunning {
        name: String,
        pid: NonZeroI32,
        path: PathBuf,
    },

    #[error("failed to parse config for {name:?} at {path}: {source}")]
    ConfigLoad {
        name: String,
        path: PathBuf,
        #[source]
        source: eyre::Report,
    },

    #[error("unsupported guest OS in {name:?}")]
    UnsupportedGuestOs { name: String },

    #[error("I/O failure")]
    Io(#[from] std::io::Error),

    #[error("libvm failure: {0}")]
    LibVm(#[from] bento_libvm::LibVmError),
}

#[derive(Debug)]
struct MigrationPlan {
    name: String,
    source_dir: PathBuf,
    target_id: MachineId,
    target_dir: PathBuf,
    spec: VmSpec,
}

fn main() -> ExitCode {
    let args = Args::parse();

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), MigrationError> {
    validate_selection(&args)?;

    let layout = Layout::from_env()?;
    let libvm = LibVm::new(layout.clone())?;
    let plans = build_plans(&layout, &args.names, args.all)?;

    if plans.is_empty() {
        println!("No old-world VMs found to migrate.");
        return Ok(());
    }

    for plan in &plans {
        println!(
            "{} -> {} ({})",
            plan.source_dir.display(),
            plan.target_dir.display(),
            plan.target_id
        );
    }

    if !args.execute {
        println!("Dry run only. Re-run with --execute to apply these migrations.");
        return Ok(());
    }

    for plan in plans {
        migrate_one(&libvm, plan)?;
    }

    Ok(())
}

fn validate_selection(args: &Args) -> Result<(), MigrationError> {
    if args.all && !args.names.is_empty() {
        return Err(MigrationError::MixedSelection);
    }
    if !args.all && args.names.is_empty() {
        return Err(MigrationError::MissingSelection);
    }
    Ok(())
}

fn build_plans(
    layout: &Layout,
    names: &[String],
    all: bool,
) -> Result<Vec<MigrationPlan>, MigrationError> {
    let old_root = layout.data_dir().to_path_buf();
    let candidate_names = if all {
        discover_old_world_names(&old_root)?
    } else {
        names.to_vec()
    };

    let mut plans = Vec::new();
    for name in candidate_names {
        let source_dir = old_root.join(&name);
        if !source_dir.is_dir() {
            return Err(MigrationError::VmNotFound {
                name,
                path: source_dir,
            });
        }

        refuse_live_pid(&name, &source_dir.join(InstanceFile::InstancedPid.as_str()))?;
        let config_path = source_dir.join(InstanceFile::Config.as_str());
        let config = InstanceConfig::from_path(&config_path).map_err(|source| {
            MigrationError::ConfigLoad {
                name: name.clone(),
                path: config_path.clone(),
                source,
            }
        })?;

        let target_id = MachineId::new();
        let spec = legacy_config_to_vm_spec(&name, &source_dir, config)?;
        let target_dir = layout.instance_dir(target_id);

        plans.push(MigrationPlan {
            name,
            source_dir,
            target_id,
            target_dir,
            spec,
        });
    }

    Ok(plans)
}

fn discover_old_world_names(old_root: &Path) -> Result<Vec<String>, MigrationError> {
    let mut names = Vec::new();
    let entries = match fs::read_dir(old_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(err) => return Err(err.into()),
    };

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };

        if matches!(name.as_str(), "images" | "instances") || name.starts_with('.') {
            continue;
        }

        if entry.path().join(InstanceFile::Config.as_str()).is_file() {
            names.push(name);
        }
    }

    names.sort();
    Ok(names)
}

fn refuse_live_pid(name: &str, pid_path: &Path) -> Result<(), MigrationError> {
    let raw = match fs::read_to_string(pid_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };

    let pid = raw.trim().parse::<i32>().ok().and_then(NonZeroI32::new);
    let Some(pid) = pid else {
        return Ok(());
    };

    match signal::kill(Pid::from_raw(pid.get()), None) {
        Ok(()) => Err(MigrationError::VmRunning {
            name: name.to_string(),
            pid,
            path: pid_path.to_path_buf(),
        }),
        Err(nix::errno::Errno::EPERM) => Err(MigrationError::VmRunning {
            name: name.to_string(),
            pid,
            path: pid_path.to_path_buf(),
        }),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(errno) => Err(std::io::Error::from_raw_os_error(errno as i32).into()),
    }
}

fn migrate_one(libvm: &LibVm, plan: MigrationPlan) -> Result<(), MigrationError> {
    if plan.target_dir.exists() {
        return Err(MigrationError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("target dir already exists: {}", plan.target_dir.display()),
        )));
    }

    if let Some(parent) = plan.target_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::rename(&plan.source_dir, &plan.target_dir)?;
    let config_path = plan.target_dir.join(InstanceFile::Config.as_str());
    let config_yaml = serde_yaml_ng::to_string(&plan.spec).map_err(std::io::Error::other)?;
    if let Err(err) = fs::write(&config_path, config_yaml) {
        let _ = fs::rename(&plan.target_dir, &plan.source_dir);
        return Err(err.into());
    }

    let _ = fs::remove_file(plan.target_dir.join(InstanceFile::InstancedSocket.as_str()));

    if let Err(err) = libvm.register_existing(plan.target_id, plan.spec, plan.target_dir.clone()) {
        let _ = fs::rename(&plan.target_dir, &plan.source_dir);
        return Err(err.into());
    }

    println!("migrated {} -> {}", plan.name, plan.target_id);
    Ok(())
}

fn legacy_config_to_vm_spec(
    name: &str,
    source_dir: &Path,
    config: InstanceConfig,
) -> Result<VmSpec, MigrationError> {
    let engine = config.engine.unwrap_or_else(default_engine_type);
    let network = resolve_network_mode(engine, config.network.as_ref());

    let mut disks = Vec::new();
    let mut has_root_disk = false;
    for disk in config.disks {
        let kind = match disk.role.unwrap_or(DiskRole::Data) {
            DiskRole::Root => {
                has_root_disk = true;
                DiskKind::Root
            }
            DiskRole::Data => DiskKind::Data,
        };
        disks.push(Disk {
            path: relativize_to_instance_dir(source_dir, &disk.path),
            kind,
            read_only: disk.read_only.unwrap_or(false),
        });
    }
    if !has_root_disk && source_dir.join(InstanceFile::RootDisk.as_str()).is_file() {
        disks.insert(
            0,
            Disk {
                path: PathBuf::from(InstanceFile::RootDisk.as_str()),
                kind: DiskKind::Root,
                read_only: false,
            },
        );
    }

    Ok(VmSpec {
        version: 1,
        name: name.to_string(),
        platform: Platform {
            guest_os: match config.os.unwrap_or(LegacyGuestOs::Linux) {
                LegacyGuestOs::Linux => GuestOs::Linux,
                _ => {
                    return Err(MigrationError::UnsupportedGuestOs {
                        name: name.to_string(),
                    })
                }
            },
            architecture: host_architecture(),
            backend: match engine {
                EngineType::VZ => Backend::Vz,
                EngineType::Firecracker => Backend::Firecracker,
                EngineType::CloudHypervisor => Backend::CloudHypervisor,
            },
        },
        resources: Resources {
            cpus: config.cpus.unwrap_or(1).max(1) as u8,
            memory_mib: config.memory.unwrap_or(512).max(1) as u32,
        },
        boot: Boot {
            kernel: config
                .kernel_path
                .map(|path| relativize_to_instance_dir(source_dir, &path)),
            initramfs: config
                .initramfs_path
                .map(|path| relativize_to_instance_dir(source_dir, &path)),
            kernel_cmdline: Vec::new(),
            bootstrap: config.bootstrap.map(|_| Bootstrap {
                cloud_init: config
                    .userdata_path
                    .as_ref()
                    .map(|path| relativize_to_instance_dir(source_dir, path)),
            }),
        },
        storage: Storage { disks },
        mounts: config
            .mounts
            .into_iter()
            .map(|mount| Mount {
                source: mount.location,
                tag: String::new(),
                read_only: !mount.writable,
            })
            .collect(),
        network: Network {
            mode: match network {
                LegacyNetworkMode::VzNat => NetworkMode::User,
                LegacyNetworkMode::None => NetworkMode::None,
                LegacyNetworkMode::Bridged => NetworkMode::Bridged,
                LegacyNetworkMode::Cni => NetworkMode::User,
            },
        },
        guest: Guest {
            profiles: config.profiles,
            capabilities: Capabilities {
                ssh: config.capabilities.ssh.enabled,
                docker: false,
                dns: config.capabilities.dns.enabled,
                forward: config.capabilities.forward.enabled,
            },
        },
        host: Host {
            nested_virtualization: config.nested_virtualization.unwrap_or(false),
            rosetta: config.rosetta.unwrap_or(false),
        },
    })
}

fn relativize_to_instance_dir(instance_dir: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(instance_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn host_architecture() -> Architecture {
    match std::env::consts::ARCH {
        "aarch64" => Architecture::Aarch64,
        _ => Architecture::X86_64,
    }
}
