use std::fmt::{Display, Formatter};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

use bento_runtime::instance::{InstanceFile, InstanceStatus};
use bento_runtime::instance_manager::{InstanceError, InstanceManager, NixDaemon};
use clap::Args;
use eyre::{bail, Context};

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,

    #[arg(long, short = 'u', default_value = "root")]
    pub user: String,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} --user {}", self.name, self.user)
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

        let exe = std::env::current_exe().context("resolve bentoctl binary path")?;
        let known_hosts = inst
            .file(InstanceFile::InstancedSocket)
            .with_file_name("known_hosts");

        let proxy_command = format!(
            "{} shell-proxy --name {} --service ssh",
            shell_quote(&exe.to_string_lossy()),
            shell_quote(&self.name)
        );

        let err = Command::new("ssh")
            // Do not read ~/.ssh/config or custom host stanzas. Keeps behavior deterministic.
            .arg("-F")
            .arg("/dev/null")
            // ProxyCommand
            .arg("-o")
            .arg(format!("ProxyCommand={proxy_command}"))
            .arg("-o")
            .arg(format!("HostKeyAlias=bento/{}", self.name))
            .arg("-o")
            .arg(format!(
                "UserKnownHostsFile={}",
                known_hosts.to_string_lossy()
            ))
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
            .arg(format!("User={}", self.user))
            .arg(self.name.clone())
            .exec();

        // Ignore user/global ssh config
        // - -F /dev/null
        //   - Do not read ~/.ssh/config or custom host stanzas. Keeps behavior deterministic.
        //
        // Auth + identity
        // - -o IdentityFile="/Users/nickvd/.lima/_config/user"
        //   - Private key Lima generated/uses for this VM.
        // - -o PreferredAuthentications=publickey
        //   - Try key auth first/only preferred method.
        // - -o BatchMode=yes
        //   - Non-interactive mode, do not prompt for passwords/passphrases.
        // - -o IdentitiesOnly=yes
        //   - Only use the specified identity file, do not spray all agent keys.
        // - -o GSSAPIAuthentication=no
        //   - Disable Kerberos/GSSAPI attempts, avoids slow or noisy fallback paths.
        //
        // Host key handling (convenience > strict security)
        // - -o StrictHostKeyChecking=no
        //   - Accept host key changes without prompting.
        // - -o UserKnownHostsFile=/dev/null
        //   - Do not store known_hosts entries.
        // - -o NoHostAuthenticationForLocalhost=yes
        //   - Relax host auth for localhost targets.
        // These are dev-convenience settings, less secure but frictionless for local VM flows.
        //
        // Transport tuning
        // - -o Compression=no
        //   - Disable SSH compression (often faster locally).
        // - -o Ciphers="^aes128-gcm@openssh.com,aes256-gcm@openssh.com"
        //   - Prefer AES-GCM ciphers first (the ^ means prepend preference order).
        // - -o LogLevel=ERROR
        //   - Suppress normal SSH chatter, only show real errors.
        //
        // Remote identity + endpoint
        // - -o User=nickvd
        //   - Login user inside guest.
        // - -p 50301
        //   - Connect to forwarded host TCP port 50301.
        // - 127.0.0.1
        //   - Connects to local loopback, not directly to guest IP.
        //
        // So Lima here is using localhost TCP forwarding, not your VSOCK relay pattern.
        //
        // Connection multiplexing
        // - -o ControlMaster=auto
        //   - Reuse/create a master SSH connection for this target.
        // - -o ControlPath="/Users/nickvd/.lima/docker/ssh.sock"
        //   - Unix socket path for control channel.
        // - -o ControlPersist=yes
        //   - Keep master connection alive in background after session exits.
        // This is a major UX win, subsequent shell/exec/copy commands are much faster.
        //
        // TTY + env
        // - -t
        //   - Force pseudo-terminal allocation for interactive shell behavior.
        // - -o SendEnv=COLORTERM
        //   - Pass COLORTERM to guest for terminal color capability consistency.
        //
        // Remote command
        // - -- cd /Users/nickvd/Projects/bentobox || cd /Users/nickvd ; exec "$SHELL" --login
        //   - -- ends ssh options, following text runs on remote shell.
        //   - Try to enter host-equivalent project dir in guest, fallback to home.
        //   - exec "$SHELL" --login replaces command shell with login shell.

        if err.kind() == io::ErrorKind::NotFound {
            bail!("`ssh` command not found. install OpenSSH client and retry")
        }

        bail!("failed to execute ssh: {err}")
    }
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}
