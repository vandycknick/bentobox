use std::sync::Arc;

use crate::backend::{self, Backend};
use crate::stream::{SerialStream, VsockStream};
use crate::types::{
    MachineError, MachineSpec, MachineState, MachineStateReceiver, ResolvedMachineSpec,
};

pub struct Machine;

#[derive(Clone)]
pub struct MachineInstance {
    inner: Arc<MachineInstanceInner>,
}

#[derive(Debug)]
struct MachineInstanceInner {
    spec: ResolvedMachineSpec,
    backend: Backend,
}

impl std::fmt::Debug for MachineInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineInstance")
            .field("id", &self.inner.spec.id.as_str())
            .finish()
    }
}

impl Machine {
    pub fn validate(spec: &MachineSpec) -> Result<(), MachineError> {
        let resolved = spec.clone().resolve()?;
        backend::validate(&resolved)
    }

    pub fn prepare(spec: &MachineSpec) -> Result<(), MachineError> {
        let resolved = spec.clone().resolve()?;
        backend::prepare(&resolved)
    }

    pub async fn create(spec: MachineSpec) -> Result<MachineInstance, MachineError> {
        let spec = spec.resolve()?;
        let backend = backend::create_backend(&spec)?;
        Ok(MachineInstance {
            inner: Arc::new(MachineInstanceInner { spec, backend }),
        })
    }
}

impl MachineInstance {
    pub async fn state(&self) -> Result<MachineState, MachineError> {
        self.inner.backend.state().await
    }

    pub async fn start(&self) -> Result<(), MachineError> {
        self.inner.backend.start().await
    }

    pub fn subscribe_state(&self) -> MachineStateReceiver {
        self.inner.backend.subscribe_state()
    }

    pub async fn stop(&self) -> Result<(), MachineError> {
        self.inner.backend.stop().await
    }

    pub async fn open_vsock(&self, port: u32) -> Result<VsockStream, MachineError> {
        let raw = self.inner.backend.open_vsock(port).await?;
        VsockStream::from_raw(raw).map_err(MachineError::from)
    }

    pub async fn open_serial(&self) -> Result<SerialStream, MachineError> {
        let raw = self.inner.backend.open_serial().await?;
        SerialStream::from_raw(raw).map_err(MachineError::from)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{Machine, MachineInstance};
    use crate::types::{MachineConfig, MachineId, MachineSpec, NetworkMode};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn spec(id: &str, cpus: Option<usize>) -> MachineSpec {
        let dir = temp_dir(id);
        fs::create_dir_all(&dir).expect("test dir should be creatable");
        fs::write(dir.join("kernel"), b"kernel").expect("kernel should be creatable");
        fs::write(dir.join("initramfs"), b"initramfs").expect("initramfs should be creatable");

        MachineSpec {
            id: MachineId::from(id),
            kind: None,
            config: MachineConfig {
                cpus,
                memory_mib: Some(1024),
                machine_directory: dir.clone(),
                kernel_path: Some(dir.join("kernel")),
                initramfs_path: Some(dir.join("initramfs")),
                machine_identifier_path: Some(dir.join("machine-id")),
                network: NetworkMode::None,
                ..MachineConfig::new()
            },
        }
    }

    async fn create(spec: MachineSpec) -> MachineInstance {
        Machine::create(spec).await.expect("create machine")
    }

    fn unique_id(prefix: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        format!("{prefix}-{}-{now}", std::process::id())
    }

    fn temp_dir(id: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bento-machine-test-{id}"))
    }

    async fn cleanup(id: &str, machine: &MachineInstance) {
        let _ = machine.stop().await;
        let _ = fs::remove_dir_all(temp_dir(id));
    }

    #[tokio::test]
    async fn create_returns_distinct_instances_for_same_spec() {
        let id = unique_id("distinct-instances");

        let first = create(spec(&id, Some(2))).await;
        let second = create(spec(&id, Some(2))).await;

        assert!(!std::sync::Arc::ptr_eq(&first.inner, &second.inner));

        cleanup(&id, &first).await;
        let _ = second.stop().await;
    }

    #[tokio::test]
    async fn stop_is_explicit_and_idempotent() {
        let id = unique_id("stop-explicit");

        let machine = create(spec(&id, Some(2))).await;

        assert!(machine.stop().await.is_ok());
        assert!(machine.stop().await.is_ok());

        let _ = fs::remove_dir_all(temp_dir(&id));
    }
}
