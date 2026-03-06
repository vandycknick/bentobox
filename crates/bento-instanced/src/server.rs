use std::path::Path;
use std::sync::Arc;

use bento_machine::MachineHandle;
use eyre::Context;

use crate::control::handle_client;
use crate::serial::SerialRuntime;
use crate::state::InstanceStore;

#[derive(Clone)]
pub(crate) struct InstanceServer {
    machine: MachineHandle,
    serial_runtime: Arc<SerialRuntime>,
    store: Arc<InstanceStore>,
}

impl InstanceServer {
    pub(crate) fn new(
        machine: MachineHandle,
        serial_runtime: Arc<SerialRuntime>,
        store: Arc<InstanceStore>,
    ) -> Self {
        Self {
            machine,
            serial_runtime,
            store,
        }
    }

    pub(crate) fn listen(
        &self,
        path: &Path,
    ) -> eyre::Result<tokio::task::JoinHandle<eyre::Result<()>>> {
        let listener = tokio::net::UnixListener::bind(path)?;
        let server = self.clone();
        Ok(tokio::spawn(async move { server.run(listener).await }))
    }

    async fn run(self, listener: tokio::net::UnixListener) -> eyre::Result<()> {
        loop {
            let (stream, _) = listener
                .accept()
                .await
                .context("accept control socket connection")?;

            let server = self.clone();
            tokio::spawn(async move {
                if let Err(err) = server.handle(stream).await {
                    tracing::warn!(error = %err, "shell control request failed");
                }
            });
        }
    }

    async fn handle(&self, stream: tokio::net::UnixStream) -> eyre::Result<()> {
        handle_client(
            stream,
            self.machine.clone(),
            self.serial_runtime.clone(),
            self.store.clone(),
        )
        .await
    }
}
