use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use bento_runtime::instance_manager::Launcher;
use nix::unistd::setsid;

pub struct NixLauncher {
    command: Command,
}

impl NixLauncher {
    pub fn new(exe: impl AsRef<OsStr>) -> Self {
        let mut command = Command::new(exe.as_ref());
        unsafe {
            command.pre_exec(|| {
                setsid()
                    .map(|_| ())
                    .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
            });
        }

        Self { command }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.command.arg(arg);
        self
    }
}

impl Launcher for NixLauncher {
    fn spawn(&mut self) -> io::Result<Child> {
        self.command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
}

pub(crate) struct NoopLauncher;

impl Launcher for NoopLauncher {
    fn spawn(&mut self) -> io::Result<Child> {
        Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
}
