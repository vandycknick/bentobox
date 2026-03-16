use std::sync::Arc;

use crate::backend;
use crate::registry::{self, MachineWorker};
use crate::stream::{SerialStream, VsockStream};
use crate::types::{MachineError, MachineExitReceiver, MachineId, MachineSpec, MachineState};

pub struct Machine;

#[derive(Clone)]
pub struct MachineHandle {
    inner: Arc<MachineWorker>,
}

impl std::fmt::Debug for MachineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineHandle")
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

    pub async fn create_or_get(spec: MachineSpec) -> Result<MachineHandle, MachineError> {
        let spec = spec.resolve()?;
        let inner = registry::create_or_get(spec)?;
        Ok(MachineHandle { inner })
    }

    pub async fn release(id: &MachineId) -> Result<(), MachineError> {
        let worker = match registry::release(id)? {
            Some(worker) => worker,
            None => return Ok(()),
        };

        let stop_result = worker.stop().await;
        let join_result = worker.join();

        match (stop_result, join_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(stop_err), Ok(())) => Err(stop_err),
            (Ok(()), Err(join_err)) => Err(join_err),
            (Err(stop_err), Err(join_err)) => Err(MachineError::Backend(format!(
                "machine release failed during stop and join: stop={stop_err}; join={join_err}"
            ))),
        }
    }
}

impl MachineHandle {
    pub async fn state(&self) -> Result<MachineState, MachineError> {
        self.ensure_active()?;
        self.inner.state().await
    }

    pub async fn start(&self) -> Result<MachineExitReceiver, MachineError> {
        self.ensure_active()?;
        self.inner.start().await
    }

    pub async fn stop(&self) -> Result<(), MachineError> {
        self.ensure_active()?;
        self.inner.stop().await
    }

    pub async fn open_vsock(&self, port: u32) -> Result<VsockStream, MachineError> {
        self.ensure_active()?;
        let raw = self.inner.open_vsock(port).await?;
        VsockStream::from_raw(raw).map_err(MachineError::from)
    }

    pub async fn open_serial(&self) -> Result<SerialStream, MachineError> {
        self.ensure_active()?;
        let raw = self.inner.open_serial().await?;
        SerialStream::from_raw(raw).map_err(MachineError::from)
    }

    fn ensure_active(&self) -> Result<(), MachineError> {
        if self.inner.is_released() {
            return Err(MachineError::MachineReleased {
                id: self.inner.spec.id.clone(),
            });
        }

        Ok(())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{Machine, MachineHandle};
    use crate::types::{MachineConfig, MachineError, MachineId, MachineSpec, NetworkMode};
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

    async fn create(spec: MachineSpec) -> MachineHandle {
        Machine::create_or_get(spec).await.expect("create machine")
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

    async fn cleanup(id: &str) {
        let _ = Machine::release(&MachineId::from(id)).await;
        let _ = fs::remove_dir_all(temp_dir(id));
    }

    #[tokio::test]
    async fn create_or_get_returns_same_handle_for_same_spec() {
        let id = unique_id("same-handle");

        let first = create(spec(&id, Some(2))).await;
        let second = create(spec(&id, Some(2))).await;

        assert!(std::sync::Arc::ptr_eq(&first.inner, &second.inner));

        cleanup(&id).await;
    }

    #[tokio::test]
    async fn create_or_get_returns_spec_mismatch_for_same_id() {
        let id = unique_id("spec-mismatch");

        let _ = create(spec(&id, Some(2))).await;

        let err = Machine::create_or_get(spec(&id, Some(4)))
            .await
            .expect_err("spec mismatch expected");

        assert!(matches!(err, MachineError::SpecMismatch { .. }));

        cleanup(&id).await;
    }

    #[tokio::test]
    async fn release_is_idempotent() {
        let id = unique_id("release-idempotent");

        let _ = create(spec(&id, Some(2))).await;

        let _ = Machine::release(&MachineId::from(id.as_str())).await;
        let second = Machine::release(&MachineId::from(id.as_str())).await;

        assert!(second.is_ok());

        let _ = fs::remove_dir_all(temp_dir(&id));
    }

    #[tokio::test]
    async fn released_handle_returns_machine_released() {
        let id = unique_id("released-handle");

        let handle = create(spec(&id, Some(2))).await;
        let _ = Machine::release(&MachineId::from(id.as_str())).await;

        let err = handle
            .state()
            .await
            .expect_err("released handle should fail");
        assert!(matches!(err, MachineError::MachineReleased { .. }));

        let _ = fs::remove_dir_all(temp_dir(&id));
    }
}
