use std::fmt::{Display, Formatter};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

use bento_instanced::daemon::NixDaemon;
use bento_runtime::images::capabilities::Capability;
use bento_runtime::instance::{InstanceFile, InstanceStatus};
use bento_runtime::instance_manager::{InstanceError, InstanceManager};
use bento_runtime::{host_user, ssh_keys};
use clap::{Args, ValueEnum};
use eyre::{bail, Context};

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
    pub fn run(&self) -> eyre::Result<()> {
        let manager = InstanceManager::new(NixDaemon::new("123"));
        let inst = manager.inspect(&self.name)?;

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
                return attach_serial_sync(&socket_path.to_string_lossy());
            }
            Some(AttachMode::Ssh) => {
                return exec_ssh_command(&self.name, self.user.as_deref());
            }
            None => {}
        }

        if !inst.capabilities().supports(Capability::Ssh) {
            if self.user.is_some() {
                eprintln!("[bentoctl] --user is ignored for serial attach");
            }
            eprintln!("[bentoctl] image has no ssh capability, using serial attach");
            let socket_path = inst.file(InstanceFile::InstancedSocket);
            return attach_serial_sync(&socket_path.to_string_lossy());
        }

        exec_ssh_command(&self.name, self.user.as_deref())
    }
}

fn attach_serial_sync(socket_path: &str) -> eyre::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for serial attach")?;
    runtime.block_on(terminal::attach_serial(socket_path))
}

fn exec_ssh_command(name: &str, user: Option<&str>) -> eyre::Result<()> {
    let exe = std::env::current_exe().context("resolve bentoctl binary path")?;

    let proxy_command = format!(
        "{} shell-proxy --name {} --service ssh",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(name)
    );
    let host_user = host_user::current_host_user().context("resolve current host user")?;
    let ssh_user = user.unwrap_or(host_user.name.as_str());
    let user_keys = ssh_keys::ensure_user_ssh_keys().context("ensure bento SSH keys")?;

    let mut command = Command::new("ssh");
    // Do not read ~/.ssh/config or custom host stanzas. Keeps behaviour deterministic.
    let err = command
        .arg("-F")
        .arg("/dev/null")
        // Auth + identity
        .arg("-o")
        .arg(format!(
            "IdentityFile={}",
            user_keys.private_key_path.to_string_lossy()
        ))
        .arg("-o")
        .arg("PreferredAuthentications=publickey") // Try key auth first/only preferred method.
        .arg("-o")
        .arg("BatchMode=yes") // Noninteractive mode, do not prompt for passwords/passphrases.
        .arg("-o")
        .arg("IdentitiesOnly=yes") // Only use the specified identity file, do not spray all agent keys.
        .arg("-o")
        .arg("GSSAPIAuthentication=no") //Disable Kerberos/GSSAPI attempts, avoids slow or noisy fallback paths.
        // ProxyCommand
        .arg("-o")
        .arg(format!("ProxyCommand={proxy_command}"))
        .arg("-o")
        .arg(format!("HostKeyAlias=bento/{}", name))
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        // Transport tuning
        .arg("-o")
        .arg("Compression=no") // Disable SSH compression (often faster locally)
        .arg("-o")
        .arg("Ciphers=\"^aes128-gcm@openssh.com,aes256-gcm@openssh.com\"") //Prefer AES-GCM ciphers first (the ^ means prepend preference order).
        .arg("-o")
        .arg("LogLevel=ERROR") // Suppress normal SSH chatter, only show real errors.
        // TTY + env
        .arg("-t")
        .arg("-o")
        .arg("SendEnv=COLORTERM")
        // Remote endpoint and identity
        .arg("-o")
        .arg(format!("User={ssh_user}"))
        .arg(name)
        .exec();

    if err.kind() == io::ErrorKind::NotFound {
        bail!("`ssh` command not found. install OpenSSH client and retry")
    }

    bail!("failed to execute ssh: {err}")
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}
