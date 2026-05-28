use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{BorrowedFd, FromRawFd, RawFd};
use std::sync::Arc;

use bento_core::{InstanceFile, Network, VmSpec};
use bento_virt::VirtualMachine;
use eyre::Context;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::context::{DaemonContext, RuntimeContext};
use crate::machine::{machine_identifier_path_from_dir, vm_spec_machine_config, VmSpecInputs};
use crate::state::{new_instance_store, Action};

pub const ENV_STARTPIPE: &str = "_VM_STARTPIPE";
pub const ENV_SYNCPIPE: &str = "_VM_SYNCPIPE";

#[derive(Clone, Copy, Debug)]
pub struct InheritedPipeFds {
    pub startpipe: Option<RawFd>,
    pub syncpipe: Option<RawFd>,
}

impl InheritedPipeFds {
    pub fn from_env() -> eyre::Result<Self> {
        Ok(Self {
            startpipe: parse_env_fd(ENV_STARTPIPE)?,
            syncpipe: parse_env_fd(ENV_SYNCPIPE)?,
        })
    }

    pub fn require_for_daemon(self) -> eyre::Result<Self> {
        if self.startpipe.is_none() || self.syncpipe.is_none() {
            return Err(eyre::eyre!(
                "{ENV_STARTPIPE} and {ENV_SYNCPIPE} are required unless running with --foreground"
            ));
        }
        Ok(self)
    }

    pub fn clear_cloexec(self) -> eyre::Result<()> {
        for fd in [self.startpipe, self.syncpipe].into_iter().flatten() {
            set_cloexec(fd, false).map_err(|err| eyre::eyre!("clear CLOEXEC on fd {fd}: {err}"))?;
        }
        Ok(())
    }
}

pub struct StartGate {
    file: Option<File>,
}

impl StartGate {
    pub fn from_fd(fd: Option<RawFd>) -> io::Result<Self> {
        match fd {
            Some(fd) => {
                set_cloexec(fd, true)?;
                let file = unsafe { File::from_raw_fd(fd) };
                Ok(Self { file: Some(file) })
            }
            None => Ok(Self { file: None }),
        }
    }

    pub async fn wait_for_release(&mut self) -> io::Result<()> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };

        tokio::task::spawn_blocking(move || {
            let mut byte = [0_u8; 1];
            file.read_exact(&mut byte)
        })
        .await
        .map_err(|err| io::Error::other(format!("join startpipe wait task: {err}")))??;

        Ok(())
    }
}

pub struct SyncReporter {
    file: Option<File>,
}

impl SyncReporter {
    pub fn from_fd(sync_fd: Option<RawFd>) -> io::Result<Self> {
        match sync_fd {
            Some(fd) => Self::from_sync_fd(fd),
            None => Self::from_stdout(),
        }
    }

    fn from_sync_fd(fd: RawFd) -> io::Result<Self> {
        set_cloexec(fd, true)?;
        let file = unsafe { File::from_raw_fd(fd) };
        Ok(Self { file: Some(file) })
    }

    fn from_stdout() -> io::Result<Self> {
        let borrowed = unsafe { BorrowedFd::borrow_raw(libc::STDOUT_FILENO) };
        let duplicated = nix::unistd::dup(borrowed).map_err(io::Error::other)?;
        let file = File::from(duplicated);
        Ok(Self { file: Some(file) })
    }

    pub fn report_started(&mut self) -> io::Result<()> {
        self.write_message("started\n")
    }

    pub fn report_failed(&mut self, message: &str) -> io::Result<()> {
        self.write_message(&format!("failed\t{message}\n"))
    }

    fn write_message(&mut self, message: &str) -> io::Result<()> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };
        file.write_all(message.as_bytes())?;
        file.flush()?;
        Ok(())
    }
}

pub async fn init(
    runtime: &RuntimeContext,
    machine_id: &str,
    start_gate: &mut StartGate,
) -> eyre::Result<DaemonContext> {
    let spec = load_spec(runtime)?;
    let network = load_network_runtime(runtime)?;

    tracing::info!(instance = %spec.name, "vmmon starting");
    remove_stale_socket(&runtime.file(InstanceFile::VmmonSocket))?;

    let machine_config = vm_spec_machine_config(VmSpecInputs {
        name: &spec.name,
        id: machine_id,
        data_dir: runtime.dir(),
        spec: &spec,
        network: &network,
    })?;
    let machine = VirtualMachine::new(machine_config.config)?;
    if let Some(machine_identifier) = machine_config.machine_identifier.as_ref() {
        if machine_identifier.was_generated() {
            let machine_identifier_path = machine_identifier_path_from_dir(runtime.dir());
            std::fs::write(machine_identifier_path, machine_identifier.bytes())?;
        }
    }

    let serial_console = machine.serial();
    let store = Arc::new(new_instance_store());

    store.dispatch(Action::vm_starting());
    start_gate.wait_for_release().await?;
    machine.start().await?;
    store.dispatch(Action::vm_running());

    Ok(DaemonContext {
        spec,
        machine,
        serial_console,
        store,
        shutdown: CancellationToken::new(),
    })
}

#[derive(Debug, Deserialize)]
struct NetworkRuntimeFile {
    attachment: Network,
}

fn load_spec(runtime: &RuntimeContext) -> eyre::Result<VmSpec> {
    let config_path = runtime.file(InstanceFile::Config);
    let raw = std::fs::read_to_string(&config_path)
        .wrap_err_with(|| format!("read vm spec at {}", config_path.display()))?;
    serde_yaml_ng::from_str(&raw)
        .map_err(|err| eyre::eyre!("parse vm spec at {}: {}", config_path.display(), err))
}

fn load_network_runtime(runtime: &RuntimeContext) -> eyre::Result<Network> {
    let runtime_path = runtime.dir().join("net/runtime.json");
    let raw = match std::fs::read_to_string(&runtime_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Network::None),
        Err(err) => {
            return Err(err)
                .wrap_err_with(|| format!("read network runtime at {}", runtime_path.display()))
        }
    };
    let runtime: NetworkRuntimeFile = serde_json::from_str(&raw)
        .wrap_err_with(|| format!("parse network runtime at {}", runtime_path.display()))?;
    Ok(runtime.attachment)
}

fn remove_stale_socket(path: &std::path::Path) -> eyre::Result<()> {
    if let Err(err) = std::fs::remove_file(path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err).context(format!("remove stale socket {}", path.display()));
        }
    }

    Ok(())
}

fn parse_env_fd(name: &str) -> eyre::Result<Option<RawFd>> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(None);
    };
    let raw = raw
        .into_string()
        .map_err(|_| eyre::eyre!("{name} is not valid UTF-8"))?;
    if raw.is_empty() {
        return Err(eyre::eyre!("{name} is empty"));
    }
    let fd = raw
        .parse::<RawFd>()
        .map_err(|err| eyre::eyre!("parse {name}={raw:?}: {err}"))?;
    if fd < 0 {
        return Err(eyre::eyre!("{name} must be a non-negative fd"));
    }

    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    nix::fcntl::fcntl(borrowed, nix::fcntl::FcntlArg::F_GETFD)
        .map_err(|err| eyre::eyre!("validate {name} fd {fd}: {err}"))?;

    Ok(Some(fd))
}

fn set_cloexec(fd: RawFd, enabled: bool) -> io::Result<()> {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let flags =
        nix::fcntl::fcntl(borrowed, nix::fcntl::FcntlArg::F_GETFD).map_err(io::Error::other)?;
    let mut fd_flags = nix::fcntl::FdFlag::from_bits_retain(flags);
    if enabled {
        fd_flags.insert(nix::fcntl::FdFlag::FD_CLOEXEC);
    } else {
        fd_flags.remove(nix::fcntl::FdFlag::FD_CLOEXEC);
    }
    nix::fcntl::fcntl(borrowed, nix::fcntl::FcntlArg::F_SETFD(fd_flags))
        .map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::fd::IntoRawFd;

    use nix::unistd::pipe;

    use crate::startup::{StartGate, SyncReporter};

    #[tokio::test]
    async fn start_gate_waits_for_release_byte() {
        let (read_fd, write_fd) = pipe().expect("create pipe");
        let mut gate = StartGate::from_fd(Some(read_fd.into_raw_fd())).expect("open start gate");

        let waiter = tokio::spawn(async move { gate.wait_for_release().await });

        let mut write_file = std::fs::File::from(write_fd);
        write_file.write_all(&[1]).expect("write release byte");
        drop(write_file);

        waiter
            .await
            .expect("join wait task")
            .expect("wait for release");
    }

    #[test]
    fn sync_reporter_writes_started_once() {
        let (read_fd, write_fd) = pipe().expect("create pipe");
        let mut reporter =
            SyncReporter::from_fd(Some(write_fd.into_raw_fd())).expect("open sync reporter");

        reporter.report_started().expect("report started");

        let mut file = std::fs::File::from(read_fd);
        let mut message = String::new();
        file.read_to_string(&mut message).expect("read message");
        assert_eq!(message, "started\n");
    }

    #[test]
    fn sync_reporter_writes_failed_once() {
        let (read_fd, write_fd) = pipe().expect("create pipe");
        let mut reporter =
            SyncReporter::from_fd(Some(write_fd.into_raw_fd())).expect("open sync reporter");

        reporter.report_failed("vz failed").expect("report failure");

        let mut file = std::fs::File::from(read_fd);
        let mut message = String::new();
        file.read_to_string(&mut message).expect("read message");
        assert_eq!(message, "failed\tvz failed\n");
    }
}
