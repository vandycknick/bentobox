use std::fmt::{Display, Formatter};
use std::io::Read;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bento_runtime::image_store::ImageStore;
use bento_runtime::instance::{
    InstanceFile, InstanceStatus, MountConfig, NetworkConfig, NetworkMode,
};
use bento_runtime::instance_control::{
    ControlErrorCode, ControlRequest, ControlResponse, ControlResponseBody,
    CONTROL_PROTOCOL_VERSION, SERVICE_SERIAL,
};
use bento_runtime::instance_manager::{
    InstanceCreateOptions, InstanceError, InstanceManager, NixDaemon,
};
use clap::{Args, ValueEnum};
use eyre::{bail, Context};
use serde_json::Map;

use crate::commands::shell;

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
pub enum AttachMode {
    Ssh,
    Serial,
}

#[derive(Args, Debug)]
pub struct Cmd {
    pub name: String,

    #[arg(long, value_enum, default_value_t = AttachMode::Ssh)]
    pub attach: AttachMode,

    #[arg(long, short = 'u')]
    pub user: Option<String>,

    #[arg(long)]
    pub keep: bool,

    #[arg(long, default_value_t = 1, help = "number of virtual CPUs")]
    pub cpus: u8,

    #[arg(
        long,
        default_value_t = 512,
        help = "virtual machine RAM size in mibibytes"
    )]
    pub memory: u32,

    #[arg(long, help = "Path to a custom kernel, only works for Linux.")]
    pub kernel: Option<PathBuf>,

    #[arg(
        long = "initramfs",
        visible_alias = "initrd",
        help = "Path to a custom initramfs image, only works for Linux."
    )]
    pub initramfs: Option<PathBuf>,

    #[arg(long, help = "Base image name or OCI reference")]
    pub image: Option<String>,

    #[arg(
        long = "disk",
        value_name = "PATH",
        help = "Path to an existing disk image"
    )]
    pub disks: Vec<PathBuf>,

    #[arg(long = "mount", value_name = "PATH:ro|rw", value_parser = parse_mount_arg)]
    pub mounts: Vec<MountConfig>,

    #[arg(long, value_name = "MODE", value_parser = parse_network_mode)]
    pub network: Option<NetworkMode>,
}

impl Display for Cmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "run")
    }
}

impl Cmd {
    pub fn run(&self) -> eyre::Result<()> {
        let name = self.name.clone();

        let exe = std::env::current_exe().context("resolve bentoctl binary path")?;
        let daemon = NixDaemon::new(exe)
            .arg("instanced")
            .arg("--name")
            .arg(&self.name);
        let mut manager = InstanceManager::new(daemon);
        let mut store = ImageStore::open()?;

        let kernel_path = resolve_optional_path(self.kernel.as_deref(), "kernel")?;
        let initramfs_path = resolve_optional_path(self.initramfs.as_deref(), "initramfs")?;
        let disk_paths = resolve_existing_paths(&self.disks, "disk")?;

        let options = InstanceCreateOptions::default()
            .with_cpus(self.cpus)
            .with_memory(self.memory)
            .with_kernel(kernel_path)
            .with_initramfs(initramfs_path)
            .with_disks(disk_paths)
            .with_mounts(self.mounts.clone())
            .with_network(self.network.map(|mode| NetworkConfig { mode }));

        let selected_image = self
            .image
            .as_deref()
            .map(|image_arg| -> eyre::Result<_> {
                Ok(match store.resolve(image_arg)? {
                    Some(image) => image,
                    None => store.pull(image_arg, None)?,
                })
            })
            .transpose()?;

        let mut created = false;
        let mut started = false;

        let run_result = (|| -> eyre::Result<()> {
            let inst = manager.create(&name, options)?;
            created = true;

            if let Some(image) = &selected_image {
                store.clone_base_image(&image, &inst.file(InstanceFile::RootDisk))?;
            }

            manager.start(&inst)?;

            started = true;

            match self.attach {
                AttachMode::Ssh => attach_ssh(&name, self.user.as_deref()),
                AttachMode::Serial => attach_serial(&name),
            }
        })();

        let cleanup_result = if self.keep {
            Ok(())
        } else {
            cleanup_run_instance(&manager, &name, created, started)
        };

        if let Err(run_err) = run_result {
            if let Err(cleanup_err) = cleanup_result {
                return Err(run_err).context(format!("cleanup failed: {cleanup_err}"));
            }
            return Err(run_err);
        }

        cleanup_result
    }
}

fn cleanup_run_instance(
    manager: &InstanceManager<NixDaemon>,
    name: &str,
    created: bool,
    started: bool,
) -> eyre::Result<()> {
    if !created {
        return Ok(());
    }

    let inst = manager.inspect(name)?;

    if started && inst.status() == InstanceStatus::Running {
        match manager.stop(&inst) {
            Ok(()) => {}
            Err(InstanceError::InstanceNotRunning { .. }) => {}
            Err(err) => return Err(err.into()),
        }
    }

    std::thread::sleep(Duration::from_millis(200));

    let inst = manager.inspect(name)?;
    manager.delete(&inst)?;
    Ok(())
}

fn attach_serial(name: &str) -> eyre::Result<()> {
    let manager = InstanceManager::new(NixDaemon::new("123"));
    let inst = manager.inspect(name)?;
    let socket_path = inst.file(InstanceFile::InstancedSocket);

    let mut stream = match UnixStream::connect(&socket_path) {
        Ok(stream) => stream,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "instanced_unreachable: control socket {} is missing, make sure the VM is running",
                socket_path.display()
            )
        }
        Err(err) => return Err(err).context(format!("connect {}", socket_path.display())),
    };

    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("set control socket read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .context("set control socket write timeout")?;

    let mut options = Map::new();
    options.insert(
        "access".to_string(),
        serde_json::Value::String("interactive".to_string()),
    );

    ControlRequest::v1_open_service_with_options("run-serial", SERVICE_SERIAL, options)
        .write_to(&mut stream)
        .context("write serial request")?;

    loop {
        let response = ControlResponse::read_from(&mut stream).context("read serial response")?;

        if response.version != CONTROL_PROTOCOL_VERSION {
            bail!(
                "unsupported_version: daemon returned protocol version {}, expected {}",
                response.version,
                CONTROL_PROTOCOL_VERSION
            );
        }

        match response.body {
            ControlResponseBody::Opened => {
                stream
                    .set_read_timeout(None)
                    .context("clear control socket read timeout")?;
                stream
                    .set_write_timeout(None)
                    .context("clear control socket write timeout")?;
                return proxy_serial_stdio(stream);
            }
            ControlResponseBody::Starting { .. } => continue,
            ControlResponseBody::Error { code, message } => {
                bail!("{}", render_control_error(&code, &message))
            }
            ControlResponseBody::Services { .. } => {
                bail!("invalid_response: expected opened response for service request")
            }
        }
    }
}

fn attach_ssh(name: &str, user: Option<&str>) -> eyre::Result<()> {
    let mut command = shell::build_ssh_command(name, user)?;
    let status = command.status().context("run ssh client")?;
    if status.success() {
        return Ok(());
    }

    match status.code() {
        Some(code) => bail!("ssh exited with status code {code}"),
        None => bail!("ssh terminated by signal"),
    }
}

fn render_control_error(code: &ControlErrorCode, message: &str) -> String {
    match code {
        ControlErrorCode::ServiceUnavailable => {
            format!("service_unavailable: {message}. ensure guest service is running")
        }
        ControlErrorCode::UnknownService => {
            format!("unknown_service: {message}. try a supported service like 'serial'")
        }
        ControlErrorCode::UnsupportedVersion => {
            format!(
                "unsupported_version: {message}. update bentoctl/instanced to matching versions"
            )
        }
        ControlErrorCode::UnsupportedRequest => {
            format!("unsupported_request: {message}")
        }
        ControlErrorCode::InstanceNotRunning => {
            format!("instance_not_running: {message}")
        }
        ControlErrorCode::PermissionDenied => {
            format!("permission_denied: {message}")
        }
        ControlErrorCode::Internal => {
            format!("internal_error: {message}")
        }
    }
}

fn proxy_serial_stdio(mut stream: UnixStream) -> eyre::Result<()> {
    let _raw_terminal = RawTerminalGuard::new()?;

    let mut stream_write = stream.try_clone().context("clone serial relay stream")?;
    let input = std::thread::spawn(move || -> std::io::Result<()> {
        let stdin_fd = std::io::stdin().as_fd().try_clone_to_owned()?;
        let mut stdin_file = std::fs::File::from(stdin_fd);
        let mut buf = [0u8; 1024];

        loop {
            let n = stdin_file.read(&mut buf)?;
            if n == 0 {
                break;
            }

            let chunk = &buf[..n];
            if chunk.contains(&0x1d) {
                let filtered: Vec<u8> = chunk.iter().copied().filter(|b| *b != 0x1d).collect();
                if !filtered.is_empty() {
                    stream_write.write_all(&filtered)?;
                }
                let _ = stream_write.shutdown(std::net::Shutdown::Write);
                break;
            }

            stream_write.write_all(chunk)?;
        }

        Ok(())
    });

    let stdout_fd = std::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .context("dup stdout fd")?;
    let mut stdout_file = std::fs::File::from(stdout_fd);
    let _ = std::io::copy(&mut stream, &mut stdout_file).context("relay serial output")?;
    stdout_file.flush().context("flush serial output")?;

    match input.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err).context("relay serial input"),
        Err(_) => bail!("serial relay thread panicked"),
    }
}

struct RawTerminalGuard {
    fd: std::os::fd::OwnedFd,
    original: libc::termios,
    enabled: bool,
}

impl RawTerminalGuard {
    fn new() -> eyre::Result<Self> {
        let stdin = std::io::stdin();
        let fd = stdin.as_fd().try_clone_to_owned().context("dup stdin fd")?;

        if unsafe { libc::isatty(fd.as_raw_fd()) } == 0 {
            return Ok(Self {
                fd,
                original: unsafe { std::mem::zeroed() },
                enabled: false,
            });
        }

        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd.as_raw_fd(), &mut original) } != 0 {
            return Err(std::io::Error::last_os_error()).context("tcgetattr stdin");
        }

        let mut raw = original;
        raw.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd.as_raw_fd(), libc::TCSAFLUSH, &raw) } != 0 {
            return Err(std::io::Error::last_os_error()).context("tcsetattr stdin raw");
        }

        Ok(Self {
            fd,
            original,
            enabled: true,
        })
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ =
                unsafe { libc::tcsetattr(self.fd.as_raw_fd(), libc::TCSAFLUSH, &self.original) };
        }
    }
}

fn unix_ts() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn resolve_optional_path(path: Option<&Path>, kind: &str) -> eyre::Result<Option<PathBuf>> {
    let Some(path) = path else {
        return Ok(None);
    };

    Ok(Some(resolve_existing_path(path, kind)?))
}

fn resolve_existing_paths(paths: &[PathBuf], kind: &str) -> eyre::Result<Vec<PathBuf>> {
    paths
        .iter()
        .map(|path| resolve_existing_path(path, kind))
        .collect()
}

fn resolve_existing_path(path: &Path, kind: &str) -> eyre::Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let abs = std::fs::canonicalize(&abs)
        .context(format!("{kind} path does not exist: {}", abs.display()))?;

    Ok(abs)
}

fn parse_mount_arg(input: &str) -> Result<MountConfig, String> {
    let (location, mode) = input
        .rsplit_once(':')
        .ok_or_else(|| "invalid mount, expected PATH:ro|rw".to_string())?;

    if location.is_empty() {
        return Err("invalid mount, path cannot be empty".to_string());
    }

    let writable = match mode {
        "rw" => true,
        "ro" => false,
        _ => {
            return Err(format!(
                "invalid mount mode '{mode}', expected 'ro' or 'rw'"
            ))
        }
    };

    Ok(MountConfig {
        location: PathBuf::from(location),
        writable,
    })
}

fn parse_network_mode(input: &str) -> Result<NetworkMode, String> {
    match input {
        "vznat" => Ok(NetworkMode::VzNat),
        "none" => Ok(NetworkMode::None),
        "bridged" => Ok(NetworkMode::Bridged),
        "cni" => Ok(NetworkMode::Cni),
        _ => Err(format!(
            "invalid network mode '{input}', expected one of: vznat, none, bridged, cni"
        )),
    }
}
