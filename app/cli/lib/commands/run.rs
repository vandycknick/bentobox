use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Args;
use libvm::{MachineNetworkConfig, MachineRef, Memory, Runtime, DEFAULT_GUEST_READINESS_TIMEOUT};
use vm_spec::Mount;

use crate::commands::create::{
    profile_mount_to_mount, read_userdata_path, resolve_boot_assets, VmOverrideArgs,
};
use crate::commands::rootfs_image::{get_base_rootfs_image, record_base_rootfs_metadata};
use crate::commands::start_options::machine_start_options;
use crate::constants::{DEFAULT_PROFILE_NAME, PROFILE_METADATA_KEY};
use crate::context::Context;
use crate::profile::ProfileStore;
use crate::ssh;
use crate::ui::{watch_image_progress, Spinner};

const EXAMPLES: &[&str] = &[
    "bento run",
    "bento run dev",
    "bento run dev -- cargo test",
    "bento run dev --image disk:./target/rootfs.img -- cargo test",
    "bento run dev --keep-on-failure -- cargo test",
];

#[derive(Debug, Args)]
#[command(
    about = "Run an ephemeral VM from a profile or image",
    after_help = crate::help::examples(EXAMPLES)
)]
pub struct Cmd {
    /// Profile to run. Defaults to the default profile when omitted.
    #[arg(value_name = "PROFILE")]
    pub profile: Option<String>,
    /// Profile name. Alternative to the positional profile argument.
    #[arg(long = "profile")]
    pub profile_name: Option<String>,
    /// Image reference to run. Overrides the profile image when both are set.
    #[arg(long)]
    pub image: Option<String>,
    /// Keep the ephemeral VM after the shell or command exits.
    #[arg(long)]
    pub keep: bool,
    /// Keep the ephemeral VM only when the guest command exits non-zero.
    #[arg(long)]
    pub keep_on_failure: bool,
    #[command(flatten)]
    pub(crate) overrides: VmOverrideArgs,
    /// Guest command and arguments to execute after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

impl Cmd {
    pub async fn run(self, context: &mut Context) -> eyre::Result<()> {
        if self.keep_on_failure && self.command.is_empty() {
            eyre::bail!("--keep-on-failure requires a command");
        }

        let mut progress = Spinner::start("Reading", "run recipe");
        let mut resolved = self.resolve()?;
        let runtime = context.runtime().await?;
        progress.step("Finding", "boot assets");
        let data_dir = runtime.local_data_dir().to_path_buf();
        let boot_assets =
            resolve_boot_assets(&data_dir, resolved.kernel.take(), resolved.initramfs.take());
        progress.finish_clear();
        let base_rootfs = {
            let (image_progress, image_events) = ocidisk::ImageProgressSender::default_channel();
            let image_progress_task =
                watch_image_progress(resolved.image_ref.clone(), image_events);
            let image =
                get_base_rootfs_image(runtime, &resolved.image_ref, Some(image_progress)).await;
            let _ = image_progress_task.await;
            image?
        };
        record_base_rootfs_metadata(&mut resolved.metadata, &base_rootfs);
        let mut progress = Spinner::start("Creating", "ephemeral VM");
        let machine = runtime
            .machine(resolved.image_ref.clone(), base_rootfs.path)
            .labels(resolved.labels)
            .metadata(resolved.metadata)
            .maybe_cpus(resolved.cpus)
            .maybe_memory(
                resolved
                    .memory_mib
                    .map(|memory| Memory::mebibytes(u64::from(memory))),
            )
            .kernel(boot_assets.kernel)
            .maybe_initramfs(boot_assets.initramfs)
            .maybe_root_disk_size(resolved.disk_size_bytes)
            .nested_virtualization(resolved.nested_virtualization)
            .rosetta(resolved.rosetta)
            .maybe_userdata(resolved.userdata)
            .disks(resolved.disks)
            .mounts(resolved.mounts)
            .network(resolved.network)
            .create()
            .await?;
        let machine_name = machine.inspect().await?.name;
        progress.step("Starting", &machine_name);
        machine
            .start_with(machine_start_options(runtime, &machine)?)
            .await?;
        progress.step("Waiting", &machine_name);
        machine
            .wait_for_guest_running(DEFAULT_GUEST_READINESS_TIMEOUT)
            .await
            .map_err(|error| eyre::eyre!("guest readiness check failed: {error}"))?;

        progress.step("Ready", &machine_name);
        progress.finish_success("Started");

        let status = if self.command.is_empty() {
            ssh::run_remote_shell_status(&data_dir, &machine_name, None)?
        } else {
            ssh::run_remote_command(&data_dir, &machine_name, None, &self.command)?
        };
        let code = status.code().unwrap_or(1);
        let should_keep = self.keep || (self.keep_on_failure && code != 0);

        if !should_keep {
            cleanup_ephemeral(runtime, &machine_name).await?;
        }

        std::process::exit(code);
    }

    fn resolve(&self) -> eyre::Result<ResolvedRun> {
        if self.profile.is_some() && self.profile_name.is_some() {
            eyre::bail!("profile specified twice; use either positional profile or --profile");
        }

        let mut labels = BTreeMap::new();
        let mut metadata = BTreeMap::new();
        let mut mounts = Vec::<Mount>::new();
        let mut network = MachineNetworkConfig::default();
        let mut userdata = None;
        let mut cpus = None;
        let mut memory_mib = None;
        let mut disk_size_bytes = None;

        let selected_profile = self.profile.clone().or_else(|| self.profile_name.clone());
        let mut image_ref = if selected_profile.is_some() || self.image.is_none() {
            let selected = selected_profile.unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string());
            let store = ProfileStore::from_env()?;
            let named = store.resolve(&selected)?;
            network = named.profile.machine_network();
            userdata = named.profile.userdata.clone();
            cpus = named.profile.cpus();
            memory_mib = named.profile.memory_mib()?;
            disk_size_bytes = named.profile.disk_size_bytes()?;
            labels = named.profile.labels.clone();
            metadata.insert(PROFILE_METADATA_KEY.to_string(), named.name.clone());
            mounts = named.profile.resolved_mounts()?;
            named.profile.image.clone()
        } else if let Some(image) = &self.image {
            image.clone()
        } else {
            eyre::bail!("either a profile or image is required");
        };

        if let Some(image) = &self.image {
            image_ref = image.clone();
        }

        for (key, value) in &self.overrides.labels {
            labels.insert(key.clone(), value.clone());
        }
        for mount in &self.overrides.mounts {
            mounts.push(profile_mount_to_mount(mount)?);
        }
        if let Some(network_override) = self.overrides.network.clone() {
            network = network_override;
        }
        if let Some(userdata_path) = self.overrides.userdata.as_deref() {
            userdata = Some(read_userdata_path(userdata_path)?);
        }

        Ok(ResolvedRun {
            image_ref,
            labels,
            metadata,
            mounts,
            network,
            userdata,
            cpus: self.overrides.cpus.or(cpus),
            memory_mib: self.overrides.memory_mib()?.or(memory_mib),
            kernel: self.overrides.kernel.clone(),
            initramfs: self.overrides.initramfs.clone(),
            disk_size_bytes: self.overrides.disk_size_bytes()?.or(disk_size_bytes),
            nested_virtualization: self.overrides.nested_virtualization,
            rosetta: self.overrides.rosetta,
            disks: self.overrides.disks.clone(),
        })
    }
}

struct ResolvedRun {
    image_ref: String,
    labels: BTreeMap<String, String>,
    metadata: BTreeMap<String, String>,
    mounts: Vec<Mount>,
    network: MachineNetworkConfig,
    userdata: Option<String>,
    cpus: Option<u8>,
    memory_mib: Option<u32>,
    kernel: Option<PathBuf>,
    initramfs: Option<PathBuf>,
    disk_size_bytes: Option<u64>,
    nested_virtualization: bool,
    rosetta: bool,
    disks: Vec<PathBuf>,
}

async fn cleanup_ephemeral(runtime: &Runtime, name: &str) -> eyre::Result<()> {
    let machine = runtime
        .get_machine(&MachineRef::parse(name.to_string())?)
        .await?;
    match machine.stop().await {
        Ok(_) => {}
        Err(error) if error.to_string().contains("is not running") => {}
        Err(error) => return Err(error.into()),
    }
    machine.remove().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use clap::Parser;

    use crate::app::Cli;
    use crate::commands::create::resolve_boot_assets;
    use crate::commands::Command;

    #[test]
    fn run_command_parses_create_parity_overrides() {
        let cli = Cli::try_parse_from([
            "bento",
            "run",
            "dev",
            "--cpus",
            "4",
            "--memory",
            "4gb",
            "--kernel",
            "./vmlinuz",
            "--initrd",
            "./initrd.img",
            "--disk-size",
            "40gb",
            "--nested-virtualization",
            "--rosetta",
            "--userdata",
            "./user-data.yaml",
            "--disk",
            "./data.raw",
            "--mount",
            ".:/workspace:rw",
            "--network",
            "none",
            "--label",
            "env=dev",
        ])
        .expect("run command should parse");
        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };

        assert_eq!(run.profile.as_deref(), Some("dev"));
        assert_eq!(run.overrides.cpus, Some(4));
        assert_eq!(run.overrides.memory_mib().expect("memory mib"), Some(4096));
        assert_eq!(
            run.overrides.disk_size_bytes().expect("disk size bytes"),
            Some(40 * 1024 * 1024 * 1024)
        );
        assert!(run.overrides.nested_virtualization);
        assert!(run.overrides.rosetta);
        assert_eq!(run.overrides.disks.len(), 1);
        assert_eq!(run.overrides.mounts.len(), 1);
        assert_eq!(
            run.overrides.labels,
            vec![("env".to_string(), "dev".to_string())]
        );
    }

    #[test]
    fn run_command_accepts_image_override_with_profile() {
        let cli = Cli::try_parse_from([
            "bento",
            "run",
            "dev",
            "--image",
            "tar:./target/rootfs.tar",
            "--",
            "true",
        ])
        .expect("run command should parse");
        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };

        assert_eq!(run.profile.as_deref(), Some("dev"));
        assert_eq!(run.image.as_deref(), Some("tar:./target/rootfs.tar"));
    }

    #[test]
    fn run_command_leaves_default_initramfs_for_libvm_generation() {
        let cli = Cli::try_parse_from([
            "bento",
            "run",
            "dev",
            "--image",
            "disk:./target/rootfs.img",
            "--",
            "true",
        ])
        .expect("run command should parse");
        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };

        let assets = resolve_boot_assets(
            Path::new("/data/bento"),
            run.overrides.kernel.clone(),
            run.overrides.initramfs.clone(),
        );

        assert_eq!(assets.kernel, PathBuf::from("/data/bento/assets/default"));
        assert_eq!(assets.initramfs, None);
    }

    #[test]
    fn run_command_forwards_explicit_initramfs_to_libvm() {
        let cli = Cli::try_parse_from([
            "bento",
            "run",
            "dev",
            "--initrd",
            "./initrd.img",
            "--",
            "true",
        ])
        .expect("run command should parse");
        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };

        let assets = resolve_boot_assets(
            Path::new("/data/bento"),
            run.overrides.kernel.clone(),
            run.overrides.initramfs.clone(),
        );

        assert_eq!(assets.kernel, PathBuf::from("/data/bento/assets/default"));
        assert_eq!(assets.initramfs, Some(PathBuf::from("./initrd.img")));
    }

    #[test]
    fn run_command_rejects_bare_memory_and_disk_size() {
        assert!(Cli::try_parse_from(["bento", "run", "dev", "--memory", "4096"]).is_err());
        assert!(Cli::try_parse_from(["bento", "run", "dev", "--disk-size", "40"]).is_err());
    }
}
