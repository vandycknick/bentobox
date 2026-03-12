use std::path::Path;
use std::sync::Arc;

use bento_machine::MachineHandle;
use eyre::Context;
use tokio::net::UnixListener;

use crate::discovery::{ServiceRegistry, ServiceTarget};
use crate::tunnel::spawn_tunnel;

pub(crate) fn listen_host_service(
    machine: MachineHandle,
    socket_path: &Path,
    service: String,
) -> eyre::Result<tokio::task::JoinHandle<eyre::Result<()>>> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .context(format!("create host socket directory {}", parent.display()))?;
    }

    let listener = UnixListener::bind(socket_path).context(format!(
        "bind host service socket {}",
        socket_path.display()
    ))?;
    let machine = Arc::new(machine);

    Ok(tokio::spawn(async move {
        loop {
            let (stream, _) = listener
                .accept()
                .await
                .context("accept host socket connection")?;
            let machine = Arc::clone(&machine);
            let service = service.clone();

            tokio::spawn(async move {
                match ServiceRegistry::discover(&machine).await {
                    Ok(registry) => match registry.resolve(&service) {
                        Some(ServiceTarget::VsockPort(port)) => {
                            match machine.open_vsock(port).await {
                                Ok(vsock_stream) => {
                                    spawn_tunnel(stream, vsock_stream);
                                }
                                Err(err) => {
                                    tracing::warn!(error = %err, service = %service, "open vsock for host export failed");
                                }
                            }
                        }
                        Some(ServiceTarget::Serial) | None => {
                            tracing::warn!(service = %service, "host export target unavailable");
                        }
                    },
                    Err(err) => {
                        tracing::warn!(error = %err, service = %service, "guest discovery failed for host export");
                    }
                }
            });
        }
    }))
}
