use crate::runtime::RemoteRuntimeConfig;
use crate::LibVmError;

#[derive(Debug, Clone)]
pub(crate) struct RemoteRuntime {
    config: RemoteRuntimeConfig,
}

impl RemoteRuntime {
    pub(crate) fn new(config: RemoteRuntimeConfig) -> Self {
        Self { config }
    }

    pub(crate) fn unsupported<T>(&self, operation: &'static str) -> Result<T, LibVmError> {
        Err(LibVmError::RemoteRuntimeUnsupported {
            endpoint: self.config.endpoint.clone(),
            operation,
        })
    }
}
