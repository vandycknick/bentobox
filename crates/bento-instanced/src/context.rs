use std::path::PathBuf;
use std::sync::Arc;

use bento_core::{InstanceFile, VmSpec};
use bento_vmm::{SerialConsole, VirtualMachine};
use tokio_util::sync::CancellationToken;

use crate::pid_guard::PidGuard;
use crate::state::InstanceStore;

#[derive(Debug, Clone)]
pub(crate) struct VmContext {
    pub name: String,
    pub data_dir: PathBuf,
    pub spec: VmSpec,
}

impl VmContext {
    pub(crate) fn dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub(crate) fn file(&self, file: InstanceFile) -> PathBuf {
        self.data_dir.join(file.as_str())
    }
}

#[derive(Clone)]
pub struct DaemonContext {
    pub(crate) vm: VmContext,
    pub(crate) machine: VirtualMachine,
    pub(crate) serial_console: Arc<SerialConsole>,
    pub(crate) store: Arc<InstanceStore>,
    pub(crate) _pid_guard: Arc<PidGuard>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) guest_enabled: bool,
}
