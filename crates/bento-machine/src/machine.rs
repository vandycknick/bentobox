use std::sync::Arc;

use crate::registry::{self, MachineInner};
use crate::types::{
    MachineError, MachineId, MachineSpec, MachineState, OpenDeviceRequest, OpenDeviceResponse,
};

pub struct Machine;

#[derive(Clone)]
pub struct MachineHandle {
    inner: Arc<MachineInner>,
}

impl std::fmt::Debug for MachineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineHandle")
            .field("id", &self.inner.spec.id.as_str())
            .finish()
    }
}

impl Machine {
    pub async fn create_or_get(spec: MachineSpec) -> Result<MachineHandle, MachineError> {
        let spec = spec.resolve()?;
        let inner = registry::create_or_get(spec)?;
        Ok(MachineHandle { inner })
    }

    pub async fn release(id: &MachineId) -> Result<(), MachineError> {
        let machine = match registry::release(id)? {
            Some(machine) => machine,
            None => return Ok(()),
        };

        machine.stop().await
    }
}

impl MachineHandle {
    pub async fn state(&self) -> Result<MachineState, MachineError> {
        self.ensure_active()?;
        self.inner.state().await
    }

    pub async fn start(&self) -> Result<(), MachineError> {
        self.ensure_active()?;
        self.inner.start().await
    }

    pub async fn stop(&self) -> Result<(), MachineError> {
        self.ensure_active()?;
        self.inner.stop().await
    }

    pub async fn open_device(
        &self,
        request: OpenDeviceRequest,
    ) -> Result<OpenDeviceResponse, MachineError> {
        self.ensure_active()?;
        self.inner.open_device(request).await
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

#[cfg(test)]
mod tests {
    use super::{Machine, MachineHandle};
    use crate::types::{MachineConfig, MachineError, MachineId, MachineSpec};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn spec(id: &str, cpus: Option<usize>) -> MachineSpec {
        MachineSpec {
            id: MachineId::from(id),
            kind: None,
            config: MachineConfig {
                cpus,
                memory_mib: Some(1024),
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

    #[tokio::test]
    async fn create_or_get_returns_same_handle_for_same_spec() {
        let id = unique_id("same-handle");

        let first = create(spec(&id, Some(2))).await;
        let second = create(spec(&id, Some(2))).await;

        assert!(std::sync::Arc::ptr_eq(&first.inner, &second.inner));

        let _ = Machine::release(&MachineId::from(id.as_str())).await;
    }

    #[tokio::test]
    async fn create_or_get_returns_spec_mismatch_for_same_id() {
        let id = unique_id("spec-mismatch");

        let _ = create(spec(&id, Some(2))).await;

        let err = Machine::create_or_get(spec(&id, Some(4)))
            .await
            .expect_err("spec mismatch expected");

        assert!(matches!(err, MachineError::SpecMismatch { .. }));

        let _ = Machine::release(&MachineId::from(id.as_str())).await;
    }

    #[tokio::test]
    async fn release_is_idempotent() {
        let id = unique_id("release-idempotent");

        let _ = create(spec(&id, Some(2))).await;

        let _ = Machine::release(&MachineId::from(id.as_str())).await;
        let second = Machine::release(&MachineId::from(id.as_str())).await;

        assert!(second.is_ok());
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
    }
}
