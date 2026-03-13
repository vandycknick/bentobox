use std::fmt::{Display, Formatter};

use bento_runtime::extensions::BuiltinExtension;
use bento_runtime::instance::InstanceStatus;
use bento_runtime::instance_store::{InstanceError, InstanceStore};
use clap::Args;
use eyre::bail;

use crate::ssh;

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,

    #[arg(long, short = 'u')]
    pub user: Option<String>,

    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.user.as_deref() {
            Some(user) => write!(
                f,
                "{} --user {} -- {}",
                self.name,
                user,
                self.command.join(" ")
            ),
            None => write!(f, "{} -- {}", self.name, self.command.join(" ")),
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

        if !inst.extensions().is_enabled(BuiltinExtension::Ssh) {
            bail!("instance has no ssh extension, cannot run remote commands")
        }

        let status = ssh::run_remote_command(&self.name, self.user.as_deref(), &self.command)?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::commands::{BentoCtlCmd, Command};

    #[test]
    fn exec_command_parses_trailing_args() {
        let cmd = BentoCtlCmd::try_parse_from([
            "bentoctl",
            "exec",
            "arch",
            "--",
            "make",
            "kernel",
            "TRACK=stable",
            "ARCH=arm64",
        ])
        .expect("exec command should parse");

        let exec = match cmd.cmd {
            Command::Exec(cmd) => cmd,
            other => panic!("expected exec command, got {other:?}"),
        };

        assert_eq!(exec.name, "arch");
        assert_eq!(
            exec.command,
            vec!["make", "kernel", "TRACK=stable", "ARCH=arm64"]
        );
    }
}
