use bento_runtime::instance::{InstanceFile, InstanceStatus};
use bento_runtime::instance_store::{InstanceError, InstanceStore};
use clap::{Args, ValueEnum};
use std::fmt::{Display, Formatter};

use crate::ssh;
use crate::terminal;

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
pub enum AttachMode {
    Ssh,
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
            AttachMode::Ssh => write!(f, "ssh"),
            AttachMode::Serial => write!(f, "serial"),
        }
    }
}

impl Cmd {
    pub async fn run(&self, store: &InstanceStore) -> eyre::Result<()> {
        let inst = store.inspect(&self.name)?;

        if inst.status() != InstanceStatus::Running {
            return Err(InstanceError::InstanceNotRunning {
                name: self.name.clone(),
            }
            .into());
        }

        match self.attach {
            Some(AttachMode::Serial) => {
                if self.user.is_some() {
                    eprintln!("[bentoctl] --user is ignored for serial attach");
                }
                let socket_path = inst.file(InstanceFile::InstancedSocket);
                return terminal::attach_serial(&socket_path.to_string_lossy()).await;
            }
            Some(AttachMode::Ssh) => {
                return ssh::exec_remote_shell(&self.name, self.user.as_deref());
            }
            None => {}
        }

        if !inst.capabilities().ssh.enabled {
            if self.user.is_some() {
                eprintln!("[bentoctl] --user is ignored for serial attach");
            }
            eprintln!("[bentoctl] instance has no ssh capability, using serial attach");
            let socket_path = inst.file(InstanceFile::InstancedSocket);
            return terminal::attach_serial(&socket_path.to_string_lossy()).await;
        }

        ssh::exec_remote_shell(&self.name, self.user.as_deref())
    }
}
