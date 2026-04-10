use bento_libvm::{LibVm, MachineRef};
use clap::{Args, ValueEnum};
use std::fmt::{Display, Formatter};

use crate::ssh;
use crate::terminal;

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
pub enum AttachMode {
    Shell,
    Serial,
}

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,

    #[arg(long, short = 'u')]
    pub user: Option<String>,

    #[arg(long, value_enum)]
    pub attach: Option<AttachMode>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (self.user.as_deref(), self.attach) {
            (Some(user), Some(attach)) => {
                write!(f, "{} --user {} --attach {}", self.name, user, attach)
            }
            (Some(user), None) => write!(f, "{} --user {}", self.name, user),
            (None, Some(attach)) => write!(f, "{} --attach {}", self.name, attach),
            (None, None) => write!(f, "{}", self.name),
        }
    }
}

impl Display for AttachMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachMode::Shell => write!(f, "shell"),
            AttachMode::Serial => write!(f, "serial"),
        }
    }
}

impl Cmd {
    pub async fn run(&self, libvm: &LibVm) -> eyre::Result<()> {
        let machine_ref = MachineRef::parse(self.name.clone())?;
        let machine = libvm.inspect(&machine_ref)?;

        if !machine.status.is_running() {
            return Err(bento_libvm::LibVmError::MachineNotRunning {
                reference: self.name.clone(),
            }
            .into());
        }

        match self.attach {
            Some(AttachMode::Serial) => {
                if self.user.is_some() {
                    eprintln!("[bentoctl] --user is ignored for serial attach");
                }
                let stream = libvm
                    .open_serial_stream(&MachineRef::Id(machine.id))
                    .await?;
                return terminal::attach_serial_stream(stream).await;
            }
            Some(AttachMode::Shell) => {
                return ssh::exec_remote_shell(&self.name, self.user.as_deref());
            }
            None => {}
        }

        if !machine.spec.settings.guest_enabled {
            if self.user.is_some() {
                eprintln!("[bentoctl] --user is ignored for serial attach");
            }
            eprintln!("[bentoctl] instance has no guest shell, using serial attach");
            let stream = libvm
                .open_serial_stream(&MachineRef::Id(machine.id))
                .await?;
            return terminal::attach_serial_stream(stream).await;
        }

        ssh::exec_remote_shell(&self.name, self.user.as_deref())
    }
}
