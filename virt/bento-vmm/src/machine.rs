use std::sync::Arc;

use crate::backend::{self, VmBackend};
use crate::serial::SerialConsole;
use crate::types::{resolve_backend, Backend, VmConfig, VmExit, VmmError};
use crate::{VsockListener, VsockStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vmm {
    backend: Backend,
}

#[derive(Clone)]
pub struct VirtualMachine {
    name: std::string::String,
    backend: Arc<VmBackend>,
    serial_console: Arc<SerialConsole>,
}

impl std::fmt::Debug for VirtualMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualMachine")
            .field("name", &self.name)
            .finish()
    }
}

impl Vmm {
    pub fn new(backend: Backend) -> Result<Self, VmmError> {
        Ok(Self { backend })
    }

    pub async fn create(&self, config: VmConfig) -> Result<VirtualMachine, VmmError> {
        let backend = resolve_backend(self.backend)?;
        backend::validate(backend, &config)?;

        let name = config.name().to_string();
        let backend = backend::create_backend(backend, config)?;
        let serial_console = Arc::new(SerialConsole::new(backend.clone()));

        Ok(VirtualMachine {
            name,
            backend,
            serial_console,
        })
    }
}

impl VirtualMachine {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn start(&self) -> Result<(), VmmError> {
        self.backend.start().await
    }

    pub async fn stop(&self) -> Result<(), VmmError> {
        self.backend.stop().await
    }

    pub async fn restart(&self) -> Result<(), VmmError> {
        self.stop().await?;
        self.start().await
    }

    pub async fn connect_vsock(&self, port: u32) -> Result<VsockStream, VmmError> {
        self.backend.connect_vsock(port).await
    }

    /// Start listening for guest-initiated vsock connections on the host.
    ///
    /// Dropping the returned listener stops accepting new connections for the
    /// port.
    pub async fn listen_vsock(&self, port: u32) -> Result<VsockListener, VmmError> {
        self.backend.listen_vsock(port).await
    }

    pub async fn wait(&self) -> Result<VmExit, VmmError> {
        self.backend.wait().await
    }

    pub async fn try_wait(&self) -> Result<Option<VmExit>, VmmError> {
        self.backend.try_wait().await
    }

    pub fn serial(&self) -> Arc<SerialConsole> {
        self.serial_console.clone()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use crate::machine::{VirtualMachine, Vmm};
    use crate::types::{Backend, NetworkMode, VmConfig};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn config(name: &str, cpus: usize) -> VmConfig {
        let dir = temp_dir(name);
        fs::create_dir_all(&dir).expect("test dir should be creatable");
        fs::write(dir.join("kernel"), b"kernel").expect("kernel should be creatable");
        fs::write(dir.join("initramfs"), b"initramfs").expect("initramfs should be creatable");

        VmConfig::builder(name)
            .cpus(cpus)
            .memory(1024)
            .base_directory(dir.clone())
            .kernel(dir.join("kernel"))
            .initramfs(dir.join("initramfs"))
            .network(NetworkMode::None)
            .build()
    }

    async fn create(config: VmConfig) -> VirtualMachine {
        Vmm::new(Backend::Auto)
            .expect("create vmm")
            .create(config)
            .await
            .expect("create machine")
    }

    fn unique_id(prefix: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        format!("{prefix}-{}-{now}", std::process::id())
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bento-vmm-test-{name}"))
    }

    async fn cleanup(name: &str, machine: &VirtualMachine) {
        let _ = machine.stop().await;
        let _ = fs::remove_dir_all(temp_dir(name));
    }

    #[tokio::test]
    async fn create_returns_distinct_instances_for_same_config() {
        let id = unique_id("distinct-instances");
        let first = create(config(&id, 2)).await;
        let second = create(config(&id, 2)).await;

        assert!(!Arc::ptr_eq(&first.backend, &second.backend));

        cleanup(&id, &first).await;
        let _ = second.stop().await;
    }

    #[tokio::test]
    async fn stop_is_explicit_and_idempotent() {
        let id = unique_id("stop-explicit");
        let machine = create(config(&id, 2)).await;

        assert!(machine.stop().await.is_ok());
        assert!(machine.stop().await.is_ok());

        let _ = fs::remove_dir_all(temp_dir(&id));
    }
}
