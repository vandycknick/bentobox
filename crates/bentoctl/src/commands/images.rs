use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::PathBuf;

use bento_runtime::images::metadata::{
    host_arch, ImageMetadata, ImageMetadataBootstrap, ImageMetadataDefaults,
};
use bento_runtime::images::store::{human_size, image_size_bytes, ImageStore};
use bento_runtime::instance::GuestOs;
use bento_runtime::instance_store::InstanceStore;
use clap::{Args, Subcommand};
use tabwriter::TabWriter;

#[derive(Args, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    pub command: ImageSubcommand,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "images")
    }
}

#[derive(Subcommand, Debug)]
pub enum ImageSubcommand {
    List,
    Pull(PullCmd),
    Import(ImportCmd),
    Pack(PackCmd),
    Rm(RmCmd),
}

#[derive(Args, Debug)]
pub struct PullCmd {
    pub reference: String,
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct ImportCmd {
    pub path: PathBuf,
}

#[derive(Args, Debug)]
pub struct PackCmd {
    pub vm: String,
    pub reference: String,
    #[arg(long)]
    pub include_kernel: bool,
    #[arg(long, visible_alias = "include-initramfs")]
    pub include_initrd: bool,
    #[arg(long, value_name = "PATH")]
    pub outfile: Option<PathBuf>,
    #[arg(long)]
    pub debug: bool,
}

#[derive(Args, Debug)]
pub struct RmCmd {
    pub tag: String,
}

impl Cmd {
    pub async fn run(&self) -> eyre::Result<()> {
        match &self.command {
            ImageSubcommand::List => {
                let store = ImageStore::open()?;
                print_list(&store)?
            }
            ImageSubcommand::Pull(cmd) => {
                let mut store = ImageStore::open()?;
                let rec = store.pull(&cmd.reference, cmd.name.as_deref())?;
                println!("pulled {}", rec.source_ref);
            }
            ImageSubcommand::Import(cmd) => {
                if !cmd.path.is_file() {
                    eyre::bail!("import path must point to an OCI tar archive file");
                }

                let mut store = ImageStore::open()?;
                let rec = store.import(&cmd.path)?;
                println!("imported {}", rec.source_ref);
            }
            ImageSubcommand::Pack(cmd) => {
                let store = InstanceStore::new();
                let inst = store.inspect(&cmd.vm)?;
                if inst.status() != bento_runtime::instance::InstanceStatus::Stopped {
                    eyre::bail!("instance {} must be stopped before packing", inst.name);
                }
                let root_disk = inst.root_disk()?.ok_or_else(|| {
                    eyre::eyre!("instance {} has no root disk to pack", inst.name)
                })?;
                let boot_assets = (cmd.include_kernel || cmd.include_initrd)
                    .then(|| inst.boot_assets())
                    .transpose()?;

                let metadata = ImageMetadata {
                    schema_version: 1,
                    os: match inst.config.os.unwrap_or(GuestOs::Linux) {
                        GuestOs::Linux => "linux".to_string(),
                        GuestOs::Macos => "macos".to_string(),
                    },
                    arch: host_arch().to_string(),
                    defaults: ImageMetadataDefaults {
                        cpu: inst.config.cpus.unwrap_or(1) as u8,
                        memory_mib: inst.config.memory.unwrap_or(512) as u32,
                    },
                    bootstrap: ImageMetadataBootstrap {
                        cidata_cloud_init: inst.uses_bootstrap(),
                    },
                    extensions: inst.config.extensions.clone(),
                };

                let mut annotations = std::collections::BTreeMap::new();
                annotations.insert(
                    "org.opencontainers.image.created".to_string(),
                    chrono::Utc::now().to_rfc3339(),
                );

                let mut image_store = ImageStore::open()?;
                let pack_layout = ImageStore::build_pack_layout(
                    &cmd.reference,
                    &root_disk.path,
                    &metadata,
                    cmd.include_kernel
                        .then(|| boot_assets.as_ref().map(|assets| assets.kernel.as_path()))
                        .flatten(),
                    cmd.include_initrd
                        .then(|| {
                            boot_assets
                                .as_ref()
                                .map(|assets| assets.initramfs.as_path())
                        })
                        .flatten(),
                    annotations,
                )?;

                if let Some(outfile) = &cmd.outfile {
                    ImageStore::write_oci_archive(&pack_layout.layout_root, outfile)?;
                    println!("packed archive {}", outfile.display());
                } else {
                    let rec =
                        image_store.import_pack_layout(&cmd.reference, &pack_layout.layout_root)?;
                    println!("packed {}", rec.source_ref);
                }

                if cmd.debug {
                    println!("kept work dir {}", pack_layout.work_dir.display());
                } else {
                    let _ = std::fs::remove_dir_all(&pack_layout.work_dir);
                }
            }
            ImageSubcommand::Rm(cmd) => {
                let mut store = ImageStore::open()?;
                store.remove_image(&cmd.tag)?;
                println!("removed {}", cmd.tag);
            }
        }

        Ok(())
    }
}

fn print_list(store: &ImageStore) -> eyre::Result<()> {
    let records = store.list()?;
    let mut out = TabWriter::new(std::io::stdout()).padding(2);
    writeln!(&mut out, "NAME\tID\tOS\tSIZE\tSOURCE_REF\tARCH")?;

    for rec in records {
        let size = image_size_bytes(store, &rec.image)
            .map(human_size)
            .unwrap_or_else(|_| "unknown".to_string());
        let short_id = rec.image.id.chars().take(10).collect::<String>();

        writeln!(
            &mut out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            rec.tag,
            short_id,
            rec.image.os.unwrap_or_else(|| "-".to_string()),
            size,
            rec.image.source_ref,
            rec.image.arch.unwrap_or_else(|| "-".to_string())
        )?;
    }

    out.flush()?;

    Ok(())
}
