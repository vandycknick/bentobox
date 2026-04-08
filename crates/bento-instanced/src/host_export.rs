use std::path::Path;
use std::sync::Arc;

use bento_vmm::VirtualMachine;
use eyre::Context;
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use crate::tunnel::spawn_tunnel;

pub(crate) fn listen_host_service(
    machine: VirtualMachine,
    socket_path: &Path,
    service: String,
    port: u32,
    shutdown: CancellationToken,
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
            let (stream, _) = tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!(service = %service, port, "host service listener shutting down");
                    return Ok(());
                }
                accepted = listener.accept() => {
                    accepted.context("accept host socket connection")?
                }
            };
            let machine = Arc::clone(&machine);
            let service = service.clone();

            tokio::spawn(async move {
                match machine.connect_vsock(port).await {
                    Ok(vsock_stream) => {
                        spawn_tunnel(stream, vsock_stream);
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, service = %service, port, "open vsock for host export failed");
                    }
                }
            });
        }
    }))
}
