use std::future::Future;
use std::io;
use std::sync::Arc;

use futures::StreamExt;
use rand::Rng;
use tokio::sync::{oneshot, Semaphore};
use tokio::task::JoinHandle;
use tokio_vsock::{VsockAddr, VsockListener, VsockStream, VMADDR_CID_ANY};
use tracing::{Instrument, Span};

const GUEST_SERVICE_PORT_MIN: u32 = 2000;
const GUEST_SERVICE_PORT_MAX: u32 = 8000;
const DEFAULT_BIND_ATTEMPTS: u16 = 128;

pub struct RunningServer {
    pub port: u32,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl RunningServer {
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    pub async fn shutdown_and_wait(&mut self) {
        self.shutdown();

        if let Some(task) = self.task.take() {
            if let Err(err) = task.await {
                tracing::warn!(error = %err, port = self.port, "vsock server shutdown join failed");
            }
        }
    }

    pub async fn wait(mut self) -> Result<(), tokio::task::JoinError> {
        let _keepalive = self.shutdown_tx.take();
        if let Some(task) = self.task.take() {
            task.await
        } else {
            Ok(())
        }
    }
}

pub struct VsockServer<H> {
    handler: H,
    concurrency: usize,
    span: Span,
}

impl<H> VsockServer<H> {
    pub fn create(handler: H) -> Self {
        Self {
            handler,
            concurrency: 128,
            span: Span::none(),
        }
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    pub fn with_tracing(mut self, span: Span) -> Self {
        self.span = span;
        self
    }
}

impl<H, F> VsockServer<H>
where
    H: Fn(VsockStream) -> F + Clone + Send + Sync + 'static,
    F: Future<Output = io::Result<()>> + Send + 'static,
{
    pub fn listen(self, port: Option<u32>) -> eyre::Result<RunningServer> {
        let (bound_port, listener) = bind_listener(port)?;
        let handler = self.handler;
        let concurrency = self.concurrency;
        let span = self.span;
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let task = tokio::spawn(
            async move {
                let mut incoming = listener.incoming();
                let semaphore = Arc::new(Semaphore::new(concurrency));

                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => {
                            tracing::info!("vsock server shutdown requested");
                            break;
                        }
                        next = incoming.next() => {
                            match next {
                                Some(Ok(stream)) => {
                                    let permit = match Arc::clone(&semaphore).acquire_owned().await {
                                        Ok(permit) => permit,
                                        Err(err) => {
                                            tracing::warn!(error = %err, "semaphore closed while accepting connection");
                                            break;
                                        }
                                    };

                                    let handler = handler.clone();
                                    tokio::spawn(async move {
                                        let _permit = permit;
                                        if let Err(err) = handler(stream).await {
                                            tracing::error!(error = %err, "connection handler failed");
                                        }
                                    });
                                }
                                Some(Err(err)) => {
                                    tracing::warn!(error = %err, "accept failed");
                                }
                                None => break,
                            }
                        }
                    }
                }
            }
            .instrument(span),
        );

        Ok(RunningServer {
            port: bound_port,
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
        })
    }
}

fn bind_listener(port: Option<u32>) -> eyre::Result<(u32, VsockListener)> {
    match port {
        Some(port) => {
            let listener = VsockListener::bind(VsockAddr::new(VMADDR_CID_ANY, port))
                .map_err(|err| eyre::eyre!("bind listener on {port}: {err}"))?;

            Ok((port, listener))
        }
        None => allocate_random_listener(),
    }
}

fn allocate_random_listener() -> eyre::Result<(u32, VsockListener)> {
    let mut rng = rand::rng();

    for _ in 0..DEFAULT_BIND_ATTEMPTS {
        let port = rng.random_range(GUEST_SERVICE_PORT_MIN..=GUEST_SERVICE_PORT_MAX);
        match VsockListener::bind(VsockAddr::new(VMADDR_CID_ANY, port)) {
            Ok(listener) => return Ok((port, listener)),
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => continue,
            Err(err) => {
                return Err(eyre::eyre!(
                    "bind listener on random vsock port {port}: {err}"
                ));
            }
        }
    }

    Err(eyre::eyre!(
        "failed to find available port in range {}..={} after {} attempts",
        GUEST_SERVICE_PORT_MIN,
        GUEST_SERVICE_PORT_MAX,
        DEFAULT_BIND_ATTEMPTS
    ))
}
