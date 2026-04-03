use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api::types::{
    ConsoleConfig, CpusConfig, DiskConfig, FsConfig, MemoryConfig, PayloadConfig, RngConfig,
    VmConfig, VsockConfig,
};
use crate::client::CloudHypervisorClient;
use crate::connection::DEFAULT_TIMEOUT;
use crate::error::CloudHypervisorError;
use crate::vm::{ConfiguredVsock, VirtualMachine};

#[derive(Debug)]
pub struct VirtualMachineBuilder {
    socket_path: PathBuf,
    timeout: Duration,
    config: VmConfig,
}

impl VirtualMachineBuilder {
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            timeout: DEFAULT_TIMEOUT,
            config: empty_vm_config(),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn vm_config(mut self, config: VmConfig) -> Self {
        self.config = config;
        self
    }

    pub fn cpus(mut self, cpus: CpusConfig) -> Self {
        self.config.cpus = Some(cpus);
        self
    }

    pub fn memory(mut self, memory: MemoryConfig) -> Self {
        self.config.memory = Some(memory);
        self
    }

    pub fn payload(mut self, payload: PayloadConfig) -> Self {
        self.config.payload = payload;
        self
    }

    pub fn kernel(mut self, kernel: impl Into<String>) -> Self {
        self.config.payload.kernel = Some(kernel.into());
        self
    }

    pub fn initramfs(mut self, initramfs: impl Into<String>) -> Self {
        self.config.payload.initramfs = Some(initramfs.into());
        self
    }

    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.config.payload.cmdline = Some(cmdline.into());
        self
    }

    pub fn firmware(mut self, firmware: impl Into<String>) -> Self {
        self.config.payload.firmware = Some(firmware.into());
        self
    }

    pub fn rng(mut self, rng: RngConfig) -> Self {
        self.config.rng = Some(rng);
        self
    }

    pub fn serial(mut self, serial: ConsoleConfig) -> Self {
        self.config.serial = Some(serial);
        self
    }

    pub fn console(mut self, console: ConsoleConfig) -> Self {
        self.config.console = Some(console);
        self
    }

    pub fn add_disk(mut self, disk: DiskConfig) -> Self {
        self.config.disks.push(disk);
        self
    }

    pub fn add_fs(mut self, fs: FsConfig) -> Self {
        self.config.fs.push(fs);
        self
    }

    pub fn vsock(mut self, vsock: VsockConfig) -> Self {
        self.config.vsock = Some(vsock);
        self
    }

    pub fn set_vsock(self, vsock: VsockConfig) -> Self {
        self.vsock(vsock)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn start(self) -> Result<VirtualMachine, CloudHypervisorError> {
        let client = CloudHypervisorClient::connect_with_timeout(&self.socket_path, self.timeout)?;
        let config = normalize_config(self.config)?;

        client.create_vm(config.clone()).await?;
        client.boot_vm().await?;

        let configured_vsock = match config.vsock {
            Some(vsock) => Some(ConfiguredVsock::try_from(vsock)?),
            None => None,
        };

        Ok(VirtualMachine::new(client, configured_vsock))
    }
}

fn normalize_config(mut config: VmConfig) -> Result<VmConfig, CloudHypervisorError> {
    if config.payload.kernel.is_none()
        && config.payload.firmware.is_none()
        && config.payload.igvm.is_none()
    {
        return Err(CloudHypervisorError::MissingConfiguration(
            "payload.kernel|payload.firmware|payload.igvm",
        ));
    }

    if config.cpus.is_none() {
        config.cpus = Some(default_cpus());
    }

    let fs_present = !config.fs.is_empty();

    match config.memory.as_mut() {
        Some(memory) => {
            if fs_present {
                memory.shared = true;
            }
        }
        None => {
            config.memory = Some(default_memory(fs_present));
        }
    }

    if config.rng.is_none() {
        config.rng = Some(default_rng());
    }

    Ok(config)
}

fn empty_vm_config() -> VmConfig {
    VmConfig {
        balloon: None,
        console: None,
        cpus: None,
        debug_console: None,
        devices: Vec::new(),
        disks: Vec::new(),
        fs: Vec::new(),
        generic_vhost_user: Vec::new(),
        iommu: false,
        landlock_enable: false,
        landlock_rules: Vec::new(),
        memory: None,
        net: Vec::new(),
        numa: Vec::new(),
        payload: PayloadConfig::default(),
        pci_segments: Vec::new(),
        platform: None,
        pmem: Vec::new(),
        pvpanic: false,
        rate_limit_groups: Vec::new(),
        rng: None,
        serial: None,
        tpm: None,
        vdpa: Vec::new(),
        vsock: None,
        watchdog: false,
    }
}

fn default_cpus() -> CpusConfig {
    CpusConfig {
        affinity: Vec::new(),
        boot_vcpus: non_zero_u64(1),
        core_scheduling: None,
        features: None,
        kvm_hyperv: false,
        max_phys_bits: None,
        max_vcpus: non_zero_u64(1),
        nested: false,
        topology: None,
    }
}

fn default_memory(shared: bool) -> MemoryConfig {
    MemoryConfig {
        hotplug_method: "Acpi".to_string(),
        hotplug_size: None,
        hotplugged_size: None,
        hugepage_size: None,
        hugepages: false,
        mergeable: false,
        prefault: false,
        shared,
        size: 512 * 1024 * 1024,
        thp: true,
        zones: Vec::new(),
    }
}

fn default_rng() -> RngConfig {
    RngConfig {
        iommu: false,
        src: "/dev/urandom".to_string(),
    }
}

fn non_zero_u64(value: u64) -> NonZeroU64 {
    match NonZeroU64::new(value) {
        Some(value) => value,
        None => unreachable!("non-zero constant became zero"),
    }
}

#[cfg(test)]
mod tests {
    use super::{default_memory, VirtualMachineBuilder};
    use crate::api::types::{DiskConfig, FsConfig};

    #[test]
    fn builder_defaults_memory_to_not_shared_without_fs() {
        let memory = default_memory(false);
        assert!(!memory.shared);
        assert_eq!(memory.size, 512 * 1024 * 1024);
    }

    #[test]
    fn builder_accumulates_disks_and_fs() {
        let builder = VirtualMachineBuilder::new("/tmp/ch.sock")
            .kernel("/kernel")
            .add_disk(DiskConfig {
                path: Some("/disk.img".to_string()),
                ..Default::default()
            })
            .add_fs(FsConfig {
                id: None,
                num_queues: 1,
                pci_segment: None,
                queue_size: 1024,
                socket: "/tmp/virtiofsd.sock".to_string(),
                tag: "shared0".to_string(),
            });

        assert_eq!(builder.config.disks.len(), 1);
        assert_eq!(builder.config.fs.len(), 1);
    }
}
