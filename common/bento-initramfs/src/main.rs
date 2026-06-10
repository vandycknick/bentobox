use std::error::Error;
use std::path::PathBuf;

use bento_initramfs::{write_initramfs, InitramfsFile, InitramfsOptions};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Package the Bentobox initramfs archive")]
struct Args {
    #[arg(long, value_name = "PATH")]
    init: PathBuf,
    #[arg(long, value_name = "PATH")]
    agent: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    out: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("bento-initramfs: {error}");
        let mut source = error.source();
        while let Some(error) = source {
            eprintln!("  caused by: {error}");
            source = error.source();
        }
        std::process::exit(1);
    }
}

fn run() -> bento_initramfs::Result<()> {
    let args = Args::parse();
    let mut options = InitramfsOptions::new(args.init, args.out);
    if let Some(agent) = args.agent {
        options = options.with_extra_file(InitramfsFile::new("agent/bento-agent", agent, 0o755));
    }

    write_initramfs(&options)
}
