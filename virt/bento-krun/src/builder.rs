use std::path::{Path, PathBuf};

use bento_krun_sys::{ctx, DiskFormat, KernelFormat, SyncMode};

use crate::error::{KrunBackendError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KrunConfig {
    pub cpus: u8,
    pub memory_mib: u32,
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    pub kernel_cmdline: Vec<String>,
    pub root: Option<PathBuf>,
    pub disks: Vec<Disk>,
    pub mounts: Vec<Mount>,
    pub vsock_ports: Vec<VsockPort>,
    pub console_output: Option<PathBuf>,
    pub root_disk_remount: Option<RootDiskRemount>,
    pub disable_implicit_vsock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub block_id: String,
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub tag: String,
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VsockPort {
    pub port: u32,
    pub path: PathBuf,
    pub listen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootDiskRemount {
    pub device: String,
    pub fstype: Option<String>,
    pub options: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VirtualMachineBuilder {
    config: KrunConfig,
}

impl VirtualMachineBuilder {
    pub fn new() -> Self {
        Self {
            config: KrunConfig {
                cpus: 1,
                memory_mib: 512,
                kernel: None,
                initramfs: None,
                kernel_cmdline: Vec::new(),
                root: None,
                disks: Vec::new(),
                mounts: Vec::new(),
                vsock_ports: Vec::new(),
                console_output: None,
                root_disk_remount: None,
                disable_implicit_vsock: false,
            },
        }
    }

    pub fn cpus(mut self, cpus: u8) -> Self {
        self.config.cpus = cpus;
        self
    }

    pub fn memory_mib(mut self, memory_mib: u32) -> Self {
        self.config.memory_mib = memory_mib;
        self
    }

    pub fn kernel(mut self, kernel: impl Into<PathBuf>) -> Self {
        self.config.kernel = Some(kernel.into());
        self
    }

    pub fn initramfs(mut self, initramfs: impl Into<PathBuf>) -> Self {
        self.config.initramfs = Some(initramfs.into());
        self
    }

    pub fn kernel_cmdline(mut self, args: Vec<String>) -> Self {
        self.config.kernel_cmdline = args;
        self
    }

    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.config.root = Some(root.into());
        self
    }

    pub fn disk(mut self, disk: Disk) -> Self {
        self.config.disks.push(disk);
        self
    }

    pub fn mount(mut self, mount: Mount) -> Self {
        self.config.mounts.push(mount);
        self
    }

    pub fn vsock_port(mut self, port: VsockPort) -> Self {
        self.config.vsock_ports.push(port);
        self
    }

    pub fn console_output(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.console_output = Some(path.into());
        self
    }

    pub fn root_disk_remount(
        mut self,
        device: impl Into<String>,
        fstype: Option<String>,
        options: Option<String>,
    ) -> Self {
        self.config.root_disk_remount = Some(RootDiskRemount {
            device: device.into(),
            fstype,
            options,
        });
        self
    }

    pub fn disable_implicit_vsock(mut self, disabled: bool) -> Self {
        self.config.disable_implicit_vsock = disabled;
        self
    }

    pub fn build(self) -> Result<KrunConfig> {
        validate(&self.config)?;
        Ok(self.config)
    }

    pub fn start_enter(self) -> Result<()> {
        let config = self.build()?;
        start_enter(config)
    }
}

impl Default for VirtualMachineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn start_enter(config: KrunConfig) -> Result<()> {
    validate(&config)?;
    let ctx_id = ctx::create_ctx()?;
    let configured = configure_ctx(ctx_id, &config);
    if let Err(err) = configured {
        let _ = ctx::free_ctx(ctx_id);
        return Err(err);
    }
    ctx::start_enter(ctx_id)?;
    Ok(())
}

fn configure_ctx(ctx_id: u32, config: &KrunConfig) -> Result<()> {
    ctx::set_vm_config(ctx_id, config.cpus, config.memory_mib)?;

    if let Some(root) = config.root.as_ref() {
        ctx::set_root(ctx_id, &path_string(root))?;
    }

    if let Some(kernel) = config.kernel.as_ref() {
        let cmdline = (!config.kernel_cmdline.is_empty()).then(|| config.kernel_cmdline.join(" "));
        ctx::set_kernel(
            ctx_id,
            &path_string(kernel),
            KernelFormat::Raw,
            config
                .initramfs
                .as_ref()
                .map(|path| path_string(path))
                .as_deref(),
            cmdline.as_deref(),
        )?;
    }

    for disk in &config.disks {
        ctx::add_disk3(
            ctx_id,
            &disk.block_id,
            &path_string(&disk.path),
            DiskFormat::Raw,
            disk.read_only,
            false,
            SyncMode::Relaxed,
        )?;
    }

    for mount in &config.mounts {
        ctx::add_virtiofs3(
            ctx_id,
            &mount.tag,
            &path_string(&mount.path),
            0,
            mount.read_only,
        )?;
    }

    for port in &config.vsock_ports {
        ctx::add_vsock_port2(ctx_id, port.port, &path_string(&port.path), port.listen)?;
    }

    if let Some(path) = config.console_output.as_ref() {
        ctx::set_console_output(ctx_id, &path_string(path))?;
    }

    if config.disable_implicit_vsock {
        ctx::disable_implicit_vsock(ctx_id)?;
    }

    if let Some(remount) = config.root_disk_remount.as_ref() {
        ctx::set_root_disk_remount(
            ctx_id,
            &remount.device,
            remount.fstype.as_deref(),
            remount.options.as_deref(),
        )?;
    }

    Ok(())
}

fn validate(config: &KrunConfig) -> Result<()> {
    if config.cpus == 0 {
        return Err(KrunBackendError::InvalidConfig(
            "krun requires at least one vCPU".to_string(),
        ));
    }
    if config.memory_mib == 0 {
        return Err(KrunBackendError::InvalidConfig(
            "krun requires memory_mib to be greater than zero".to_string(),
        ));
    }
    if config.kernel.is_none() && config.root.is_none() {
        return Err(KrunBackendError::InvalidConfig(
            "krun requires either a kernel or a root filesystem".to_string(),
        ));
    }
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::{Disk, VirtualMachineBuilder};
    use std::path::PathBuf;

    #[test]
    fn builder_rejects_zero_cpus() {
        let err = VirtualMachineBuilder::new()
            .cpus(0)
            .kernel("/kernel")
            .build()
            .expect_err("zero cpus should be invalid");
        assert!(err.to_string().contains("vCPU"));
    }

    #[test]
    fn builder_accepts_disks() {
        let config = VirtualMachineBuilder::new()
            .kernel("/kernel")
            .disk(Disk {
                block_id: "root".to_string(),
                path: PathBuf::from("/root.img"),
                read_only: false,
            })
            .build()
            .expect("config should be valid");
        assert_eq!(config.disks.len(), 1);
    }
}
