use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::PathBuf;

use bento_runtime::images::store::{
    default_archive_name, human_size, image_size_bytes, ImageCompression, ImageStore,
};
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
    pub name: String,
    #[arg(long)]
    pub image: PathBuf,
    #[arg(long)]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub os: String,
    #[arg(long)]
    pub arch: String,
    #[arg(long, default_value = "zstd", value_parser = ["zstd", "gzip"])]
    pub compression: String,
}

#[derive(Args, Debug)]
pub struct RmCmd {
    pub tag: String,
}

impl Cmd {
    pub fn run(&self) -> eyre::Result<()> {
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
                let mut store = ImageStore::open()?;
                let rec = store.import(&cmd.path)?;
                println!("imported {}", rec.source_ref);
            }
            ImageSubcommand::Pack(cmd) => {
                let compression = if cmd.compression == "gzip" {
                    ImageCompression::Gzip
                } else {
                    ImageCompression::Zstd
                };

                let out = cmd
                    .out
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(default_archive_name(&cmd.name)));
                let archive = ImageStore::pack_oci_archive(
                    &cmd.image,
                    &cmd.name,
                    &out,
                    &cmd.os,
                    &cmd.arch,
                    compression,
                )?;
                println!("packed {}", archive.display());
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
