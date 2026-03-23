use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api::types::{BootSource, Drive, MachineConfiguration, Vsock};
use crate::client::FirecrackerClient;
use crate::connection::DEFAULT_TIMEOUT;
use crate::error::FirecrackerError;
use crate::vm::{ConfiguredVsock, VirtualMachine};

#[derive(Debug)]
pub struct VirtualMachineBuilder {
    socket_path: PathBuf,
    timeout: Duration,
    boot_source: Option<BootSource>,
    machine_configuration: Option<MachineConfiguration>,
    drives: Vec<Drive>,
    vsock: Option<Vsock>,
}

impl VirtualMachineBuilder {
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            timeout: DEFAULT_TIMEOUT,
            boot_source: None,
            machine_configuration: None,
            drives: Vec::new(),
            vsock: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn boot_source(mut self, boot_source: BootSource) -> Self {
        self.boot_source = Some(boot_source);
        self
    }

    pub fn set_boot_source(self, boot_source: BootSource) -> Self {
        self.boot_source(boot_source)
    }

    pub fn machine_config(mut self, machine_configuration: MachineConfiguration) -> Self {
        self.machine_configuration = Some(machine_configuration);
        self
    }

    pub fn set_machine_configuration(self, machine_configuration: MachineConfiguration) -> Self {
        self.machine_config(machine_configuration)
    }

    pub fn add_drive(mut self, drive: Drive) -> Self {
        self.drives.push(drive);
        self
    }

    pub fn drive(self, drive: Drive) -> Self {
        self.add_drive(drive)
    }

    pub fn vsock(mut self, vsock: Vsock) -> Self {
        self.vsock = Some(vsock);
        self
    }

    pub fn set_vsock(self, vsock: Vsock) -> Self {
        self.vsock(vsock)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn start(self) -> Result<VirtualMachine, FirecrackerError> {
        let client = FirecrackerClient::connect_with_timeout(&self.socket_path, self.timeout)?;
        let boot_source = self
            .boot_source
            .ok_or(FirecrackerError::MissingConfiguration("boot_source"))?;
        let machine_configuration =
            self.machine_configuration
                .ok_or(FirecrackerError::MissingConfiguration(
                    "machine_configuration",
                ))?;

        client.configure_boot_source(boot_source).await?;
        client.configure_machine(machine_configuration).await?;

        for drive in self.drives {
            client.configure_drive(drive).await?;
        }

        let configured_vsock = match self.vsock {
            Some(vsock) => {
                client.configure_vsock(vsock.clone()).await?;
                Some(ConfiguredVsock::try_from(vsock)?)
            }
            None => None,
        };

        client.start_instance().await?;

        Ok(VirtualMachine::new(client, configured_vsock))
    }
}
