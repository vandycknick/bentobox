use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use bento_runtime::instance::{Instance, InstanceFile, InstanceStatus};
use eyre::Context;
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};

use crate::service_readiness;

pub struct InstancedLauncher {
    command: Command,
}

impl InstancedLauncher {
    pub fn for_instance(exe: impl AsRef<OsStr>, name: &str) -> Self {
        let mut command = Command::new(exe.as_ref());
        command.arg("instanced").arg("--name").arg(name);
        unsafe {
            command.pre_exec(|| {
                nix::unistd::setsid()
                    .map(|_| ())
                    .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
            });
        }

        Self { command }
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        self.command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
}

pub async fn launch_instance(
    launcher: &mut InstancedLauncher,
    inst: &Instance,
) -> eyre::Result<()> {
    if inst.status() == InstanceStatus::Running {
        eyre::bail!("instance {:?} is already running", inst.name);
    }

    launcher.spawn().context("spawn instanced process")?;
    wait_for_instanced_start(
        &inst.file(InstanceFile::InstancedPid),
        &inst.file(InstanceFile::InstancedTraceLog),
    )
    .await
    .context("wait for instanced start")?;

    if inst.uses_bootstrap() {
        service_readiness::wait_for_guest_running(
            &inst.file(InstanceFile::InstancedSocket),
            Duration::from_secs(60 * 10),
        )
        .await
        .map_err(|err| eyre::eyre!("guest service discovery readiness check failed: {err}"))?;
    }

    Ok(())
}

pub fn signal_instance_stop(inst: &Instance) -> eyre::Result<()> {
    let daemon_pid = inst
        .daemon_pid
        .ok_or_else(|| eyre::eyre!("instance {:?} is not running", inst.name))?;

    let pid = Pid::from_raw(daemon_pid.get());
    signal::kill(pid, Signal::SIGINT).context("signal instanced with SIGINT")?;
    Ok(())
}

async fn wait_for_instanced_start(
    pid_path: &std::path::Path,
    trace_path: &std::path::Path,
) -> io::Result<()> {
    let deadline_duration = Duration::from_secs(30);
    let deadline = Instant::now() + deadline_duration;
    let poll_interval = Duration::from_millis(50);

    loop {
        match tokio::fs::metadata(pid_path).await {
            Ok(_) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "instanced ({}) did not start up in {:?} (hint: see {})",
                    pid_path.display(),
                    deadline_duration,
                    trace_path.display(),
                ),
            ));
        }

        tokio::time::sleep(poll_interval).await;
    }
}
