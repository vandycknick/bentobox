use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use bento_core::InstanceFile;
use clap::Parser;

mod agent;
mod context;
mod endpoints;
mod lock;
mod machine;
mod net;
mod services;
mod shutdown;
mod startup;
mod state;

use crate::context::RuntimeContext;
use crate::lock::pid::PidGuard;
use crate::startup::StartupReporter;

#[derive(Parser, Debug, Clone)]
#[command(name = "vmmon", disable_help_subcommand = true)]
struct Args {
    #[arg(long = "data-dir")]
    data_dir: PathBuf,

    #[arg(long = "startup-fd")]
    startup_fd: Option<i32>,

    #[arg(long, hide = true)]
    foreground: bool,
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();

    if args.startup_fd.is_none() && !args.foreground {
        return Err(eyre::eyre!(
            "--startup-fd is required unless running with --foreground"
        ));
    }

    if !args.foreground {
        daemonize(&args)?;
    }

    let startup_reporter = StartupReporter::from(args.startup_fd)
        .map_err(|err| eyre::eyre!("open startup reporter: {err}"))?;

    let trace_path = args.data_dir.join(InstanceFile::InstancedTraceLog.as_str());

    let trace_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&trace_path)
        .map_err(|err| eyre::eyre!("open {}: {err}", trace_path.display()))?;

    let (writer, _guard) = tracing_appender::non_blocking(trace_file);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .with_writer(writer)
        .try_init()
        .map_err(|err| eyre::eyre!("initialize instanced tracing: {err}"))?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| eyre::eyre!("build tokio runtime: {err}"))?
        .block_on(run(args, startup_reporter))
}

async fn run(args: Args, startup_reporter: StartupReporter) -> eyre::Result<()> {
    let mut startup_reporter = startup_reporter;
    let runtime = RuntimeContext::new(args.data_dir.clone());
    let _guard = PidGuard::create(&runtime.file(InstanceFile::InstancedPid)).await?;

    let result = match startup::init(&runtime).await {
        Ok(ctx) => match services::start_services(&runtime, &ctx, &mut startup_reporter).await {
            Ok(handles) => shutdown::run(runtime, ctx, handles).await,
            Err(err) => Err(err),
        },
        Err(err) => Err(err),
    };

    if let Err(err) = &result {
        let full_error = format_error_chain(err);
        tracing::error!(error = %full_error, data_dir = %args.data_dir.display(), "vmmon exiting with error");
        let _ = startup_reporter.report_failed(&full_error);
    }

    result
}

fn format_error_chain(err: &eyre::Report) -> String {
    let mut parts = Vec::new();
    for cause in err.chain() {
        parts.push(cause.to_string());
    }
    parts.join(": ")
}

#[cfg(target_os = "macos")]
fn daemonize(args: &Args) -> eyre::Result<()> {
    use std::os::unix::process::CommandExt;

    if nix::unistd::getsid(None)? == nix::unistd::getpid() {
        return Ok(());
    }

    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--data-dir").arg(&args.data_dir);
    if let Some(fd) = args.startup_fd {
        cmd.arg("--startup-fd").arg(fd.to_string());
        let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        let flags = nix::fcntl::fcntl(borrowed, nix::fcntl::FcntlArg::F_GETFD)
            .map_err(|err| eyre::eyre!("fcntl F_GETFD: {err}"))?;
        let mut fd_flags = nix::fcntl::FdFlag::from_bits_retain(flags);
        fd_flags.remove(nix::fcntl::FdFlag::FD_CLOEXEC);
        nix::fcntl::fcntl(borrowed, nix::fcntl::FcntlArg::F_SETFD(fd_flags))
            .map_err(|err| eyre::eyre!("fcntl F_SETFD: {err}"))?;
    }
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
        });
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()?;
    std::process::exit(0);
}

#[cfg(not(target_os = "macos"))]
fn daemonize(_args: &Args) -> eyre::Result<()> {
    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { .. }) => std::process::exit(0),
        Ok(nix::unistd::ForkResult::Child) => {}
        Err(err) => return Err(eyre::eyre!("fork: {err}")),
    }
    nix::unistd::setsid().map_err(|err| eyre::eyre!("setsid: {err}"))?;
    Ok(())
}
