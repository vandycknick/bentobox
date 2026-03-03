use std::future::{Future, IntoFuture};
use std::io;

use futures::{future, StreamExt};
use rand::Rng;
use tokio::task::JoinHandle;
use tokio_vsock::{VsockAddr, VsockListener, VsockStream, VMADDR_CID_ANY};
use tracing::{Instrument, Span};

const GUEST_SERVICE_PORT_MIN: u32 = 2000;
const GUEST_SERVICE_PORT_MAX: u32 = 8000;
const DEFAULT_BIND_ATTEMPTS: u16 = 128;

pub struct RunningServer {
    pub port: u32,
    task: JoinHandle<()>,
}

impl IntoFuture for RunningServer {
    type Output = Result<(), tokio::task::JoinError>;
    type IntoFuture = JoinHandle<()>;

    fn into_future(self) -> Self::IntoFuture {
        self.task
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

        let task = tokio::spawn(
            async move {
                listener
                    .incoming()
                    .filter_map(|result| {
                        future::ready(match result {
                            Ok(stream) => Some(stream),
                            Err(err) => {
                                tracing::warn!(error = %err, "accept failed");
                                None
                            }
                        })
                    })
                    .for_each_concurrent(concurrency, move |stream| {
                        let handler = handler.clone();
                        async move {
                            if let Err(err) = handler(stream).await {
                                tracing::error!(error = %err, "connection handler failed");
                            }
                        }
                    })
                    .await;
            }
            .instrument(span),
        );

        Ok(RunningServer {
            port: bound_port,
            task,
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
