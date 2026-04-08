use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use bento_core::InstanceFile;
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

    #[arg(long, hide = true)]
    foreground: bool,
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();

    if !args.foreground {
        daemonize(&args)?;
    }

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

    let startup_reporter = args.startup_fd.map(StartupReporter::from_raw_fd);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| eyre::eyre!("build tokio runtime: {err}"))?
        .block_on(run(args, startup_reporter))
}

async fn run(args: Args, startup_reporter: Option<StartupReporter>) -> eyre::Result<()> {
    VmMon::new(args.data_dir, args.profiles)
        .run(startup_reporter)
        .await
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
    for profile in &args.profiles {
        cmd.arg("--profile").arg(profile);
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
