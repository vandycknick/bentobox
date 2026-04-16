use std::path::PathBuf;
use std::sync::Arc;

use bento_core::{InstanceFile, VmSpec};
use bento_vmm::{SerialConsole, VirtualMachine};
use tokio_util::sync::CancellationToken;

use crate::state::InstanceStore;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeContext {
    dir: PathBuf,
}

impl RuntimeContext {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub(crate) fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub(crate) fn file(&self, file: InstanceFile) -> PathBuf {
        self.dir.join(file.as_str())
    }
}

#[derive(Clone)]
pub struct DaemonContext {
    pub(crate) spec: VmSpec,
    pub(crate) machine: VirtualMachine,
    pub(crate) serial_console: Arc<SerialConsole>,
    pub(crate) store: Arc<InstanceStore>,
    pub(crate) shutdown: CancellationToken,
}
