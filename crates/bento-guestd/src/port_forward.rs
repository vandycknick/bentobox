use bento_core::services::{GuestServiceConfig, GuestServiceKind};
use tokio::io::copy_bidirectional;
use tokio::net::UnixStream;

use crate::server::{RunningServer, VsockServer};

pub fn start_guest_service(service: &GuestServiceConfig) -> eyre::Result<Option<RunningServer>> {
    match service.kind {
        GuestServiceKind::Shell => Ok(None),
        GuestServiceKind::UnixSocketForward => start_guest_uds_forwarder(service).map(Some),
    }
}

fn start_guest_uds_forwarder(service: &GuestServiceConfig) -> eyre::Result<RunningServer> {
    let guest_path = service.target.clone();
    let name = service.id.clone();

    VsockServer::create(move |mut stream| {
        let guest_path = guest_path.clone();
        async move {
            let mut target = UnixStream::connect(&guest_path).await?;
            let _ = copy_bidirectional(&mut stream, &mut target).await?;
            Ok(())
        }
    })
    .with_concurrency(256)
    .with_tracing(tracing::info_span!("vsock_server", service = %name))
    .listen(Some(service.port))
}
