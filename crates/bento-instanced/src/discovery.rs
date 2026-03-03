use bento_protocol::{GuestDiscoveryClient, HealthStatus, DEFAULT_DISCOVERY_PORT};
use bento_runtime::driver::{Driver, OpenDeviceRequest, OpenDeviceResponse};
use bento_runtime::instance_control::{ServiceDescriptor, SERVICE_SERIAL, SERVICE_SSH};
use eyre::Context;
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use std::collections::BTreeMap;
use std::io;
use std::io::{Read, Write};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tarpc::context;
use tarpc::serde_transport;
use tarpc::tokio_serde::formats::Bincode;
use tarpc::tokio_util::codec::LengthDelimitedCodec;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug, Clone, Copy)]
pub(crate) enum ServiceTarget {
    VsockPort(u32),
    Serial,
}

#[derive(Debug)]
pub(crate) struct ServiceRegistry {
    by_name: BTreeMap<String, ServiceTarget>,
}

impl ServiceRegistry {
    pub(crate) async fn discover(driver: &dyn Driver) -> eyre::Result<Self> {
        let mut by_name = BTreeMap::new();
        by_name.insert(SERVICE_SERIAL.to_string(), ServiceTarget::Serial);

        let vsock_fd = match driver.open_device(OpenDeviceRequest::Vsock {
            port: DEFAULT_DISCOVERY_PORT,
        })? {
            OpenDeviceResponse::Vsock { stream } => stream,
            OpenDeviceResponse::Serial { .. } => {
                eyre::bail!("driver returned serial device when opening guest discovery port")
            }
        };

        let stream = AsyncFdStream::new(std::fs::File::from(vsock_fd))
            .context("wrap discovery stream in async fd")?;
        let framed = LengthDelimitedCodec::builder().new_framed(stream);
        let transport = serde_transport::new(framed, Bincode::default());
        let client = GuestDiscoveryClient::new(tarpc::client::Config::default(), transport).spawn();

        let HealthStatus { ok } =
            tokio::time::timeout(Duration::from_secs(3), client.health(context::current()))
                .await
                .map_err(|_| eyre::eyre!("guest discovery health request timed out"))?
                .map_err(|err| eyre::eyre!("query guest discovery health failed: {err}"))?;

        if !ok {
            eyre::bail!("guest discovery service reported unhealthy");
        }

        let endpoints = tokio::time::timeout(
            Duration::from_secs(3),
            client.list_services(context::current()),
        )
        .await
        .map_err(|_| eyre::eyre!("guest discovery list_services request timed out"))?
        .map_err(|err| eyre::eyre!("query guest service list failed: {err}"))?;

        if endpoints
            .iter()
            .all(|endpoint| endpoint.name != SERVICE_SSH)
        {
            eyre::bail!("guest discovery did not report ssh service");
        }

        for endpoint in endpoints {
            by_name.insert(endpoint.name, ServiceTarget::VsockPort(endpoint.port));
        }

        Ok(Self { by_name })
    }

    pub(crate) fn resolve(&self, name: &str) -> Option<ServiceTarget> {
        self.by_name.get(name).copied()
    }

    pub(crate) fn describe(&self) -> Vec<ServiceDescriptor> {
        self.by_name
            .keys()
            .map(|name| ServiceDescriptor { name: name.clone() })
            .collect()
    }
}

#[derive(Debug)]
struct AsyncFdStream {
    inner: AsyncFd<std::fs::File>,
}

impl AsyncFdStream {
    fn new(file: std::fs::File) -> io::Result<Self> {
        set_nonblocking(&file)?;
        Ok(Self {
            inner: AsyncFd::new(file)?,
        })
    }

    fn poll_read_priv(
        &self,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let bytes =
            unsafe { &mut *(buf.unfilled_mut() as *mut [std::mem::MaybeUninit<u8>] as *mut [u8]) };

        loop {
            let mut guard = futures::ready!(self.inner.poll_read_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().read(bytes)) {
                Ok(Ok(n)) => {
                    unsafe {
                        buf.assume_init(n);
                    }
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }

    fn poll_write_priv(&self, cx: &mut TaskContext<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = futures::ready!(self.inner.poll_write_ready(cx))?;
            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(Ok(n)) => return Poll::Ready(Ok(n)),
                Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_) => continue,
            }
        }
    }
}

impl AsyncRead for AsyncFdStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_read_priv(cx, buf)
    }
}

impl AsyncWrite for AsyncFdStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_priv(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.inner.get_ref().flush()?;
        Poll::Ready(Ok(()))
    }
}

fn set_nonblocking(file: &std::fs::File) -> io::Result<()> {
    let flags = fcntl(file, FcntlArg::F_GETFL)
        .map(OFlag::from_bits_truncate)
        .map_err(|err| io::Error::other(format!("fcntl(F_GETFL) failed: {err}")))?;

    fcntl(file, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
        .map_err(|err| io::Error::other(format!("fcntl(F_SETFL, O_NONBLOCK) failed: {err}")))?;

    Ok(())
}
