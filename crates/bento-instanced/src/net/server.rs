use std::future::Future;

use bento_protocol::negotiate::Upgrade;
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::net::listener::NegotiateListener;

pub(crate) struct NegotiateServer {
    listener: UnixListener,
    shutdown: CancellationToken,
}

impl NegotiateServer {
    pub(crate) fn new(listener: UnixListener, shutdown: CancellationToken) -> Self {
        Self { listener, shutdown }
    }

    pub(crate) fn listen<H, Fut>(self, handler: H) -> JoinHandle<eyre::Result<()>>
    where
        H: Fn(UnixStream, Upgrade) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = eyre::Result<()>> + Send + 'static,
    {
        tokio::spawn(async move {
            let incoming = NegotiateListener::new(self.listener, self.shutdown);
            while let Some((stream, upgrade)) = incoming.next().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    if let Err(err) = handler(stream, upgrade).await {
                        tracing::warn!(error = %err, "shell control request failed");
                    }
                });
            }

            Ok(())
        })
    }
}
