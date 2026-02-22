use chrono::Utc;
use eyre::Context;
use serde::{Deserialize, Serialize};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::path::PathBuf;
use std::{fs, io};
use std::{fs::OpenOptions, io::Write, path::Path};

use crate::driver::{self};
use crate::{
    instance::InstanceFile,
    instance_manager::{Daemon, InstanceManager},
};

struct NopDaemon {}

impl Daemon for NopDaemon {
    fn stdin<T: Into<std::process::Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn stdout<T: Into<std::process::Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn stderr<T: Into<std::process::Stdio>>(&mut self, _: T) -> &mut Self {
        self
    }

    fn spawn(&mut self) -> std::io::Result<std::process::Child> {
        std::process::Command::new("true").spawn()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InstancedEventType {
    Running,
    Exiting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstancedEvent {
    pub timestamp: String,

    #[serde(rename = "type")]
    pub event_type: InstancedEventType,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub struct InstanceDaemon {
    name: String,
    manager: InstanceManager<NopDaemon>,
}

impl InstanceDaemon {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            manager: InstanceManager::new(NopDaemon {}),
        }
    }

    fn emit_event(&self, event: &InstancedEvent) -> eyre::Result<()> {
        let mut out = io::stdout().lock();
        let mut data = serde_json::to_vec(event).context("serialize instanced event")?;
        data.push(b'\n');
        out.write_all(&data).context("write instanced event")?;
        out.flush().context("flush instanced event")?;
        Ok(())
    }

    pub fn run(&self) -> eyre::Result<()> {
        let inst = self.manager.inspect(&self.name)?;

        // NOTE: PidFileGuard will auto drop the id.pid file at the end of the run function
        let _pid_file_guard =
            write_pid_file(&inst.file(InstanceFile::InstancedPid), std::process::id())?;

        let mut driver = driver::get_driver_for(&inst)?;

        driver.start()?;

        self.emit_event(&InstancedEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type: InstancedEventType::Running,
            message: None,
        })?;

        let mut signals = Signals::new([SIGINT, SIGTERM]).context("register signal handlers")?;
        for sig in signals.forever() {
            match sig {
                SIGINT | SIGTERM => break,
                _ => {}
            }
        }

        driver.stop()?;

        Ok(())
    }
}

#[must_use = "hold this guard for the process lifetime to keep PID file cleanup active"]
pub struct PidFileGuard {
    path: PathBuf,
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn write_pid_file(path: &Path, pid: u32) -> eyre::Result<PidFileGuard> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .context(format!("open {}", path.display()))?;

    writeln!(file, "{pid}").context("write pid")?;
    file.flush().context("flush pid")?;
    Ok(PidFileGuard {
        path: path.to_path_buf(),
    })
}
