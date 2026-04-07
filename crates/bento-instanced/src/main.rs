use std::path::PathBuf;
use std::process::ExitCode;

use bento_vmmon::daemon::VmMon;
use bento_vmmon::StartupReporter;
use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "vmmon", disable_help_subcommand = true)]
struct Args {
    #[arg(long = "data-dir")]
    data_dir: PathBuf,

    #[arg(long = "profile", value_name = "PROFILE")]
    profiles: Vec<String>,

    #[arg(long = "startup-fd")]
    startup_fd: Option<i32>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    if let Err(err) = bootstrap(args) {
        eprintln!("{err:?}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn bootstrap(args: Args) -> eyre::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| eyre::eyre!("build vmmon tokio runtime: {err}"))?;

    runtime.block_on(run(args))
}

async fn run(args: Args) -> eyre::Result<()> {
    let startup_reporter = args.startup_fd.map(StartupReporter::from_raw_fd);

    VmMon::new(args.data_dir, args.profiles)
        .run(startup_reporter)
        .await
}
