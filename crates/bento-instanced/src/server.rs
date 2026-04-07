use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bento_runtime::negotiate::{
    Accept, Negotiate, ProxyMode, Reject, RejectCode, Response, Upgrade, NEGOTIATE_PROTOCOL_VERSION,
};
use bento_runtime::profiles::ENDPOINT_SERIAL;
use bento_vmm::{spawn_serial_tunnel, SerialAccess, SerialConsole, VirtualMachine};
use eyre::Context;
use tokio::net::{UnixListener, UnixStream};

use crate::discovery::{ServiceRegistry, ServiceTarget};
use crate::services;
use crate::state::InstanceStore;
use crate::tunnel::spawn_tunnel;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_AFTER_STARTING_MS: u32 = 1000;

#[derive(Clone)]
pub(crate) struct InstanceServer {
    machine: VirtualMachine,
    serial_console: Arc<SerialConsole>,
    store: Arc<InstanceStore>,
}

impl InstanceServer {
    pub(crate) fn new(
        machine: VirtualMachine,
        serial_console: Arc<SerialConsole>,
        store: Arc<InstanceStore>,
    ) -> Self {
        Self {
            machine,
            serial_console,
            store,
        }
    }

    pub(crate) fn listen(
        &self,
        path: &Path,
    ) -> eyre::Result<tokio::task::JoinHandle<eyre::Result<()>>> {
        let listener =
            UnixListener::bind(path).context(format!("bind socket {}", path.display()))?;
        let server = self.clone();
        Ok(tokio::spawn(async move { server.run(listener).await }))
    }

    async fn run(self, listener: UnixListener) -> eyre::Result<()> {
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

    async fn handle(&self, mut stream: UnixStream) -> eyre::Result<()> {
        let request = match tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            Negotiate::read_from(&mut stream),
        )
        .await
        {
            Ok(Ok(request)) => request,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "failed to read Negotiate request");
                return Ok(());
            }
            Err(_) => {
                tracing::warn!("timed out waiting for Negotiate request");
                return Ok(());
            }
        };

        if request.protocol_version != NEGOTIATE_PROTOCOL_VERSION {
            return reject(
                &mut stream,
                request.request_id,
                RejectCode::UnsupportedProtocol,
                format!(
                    "Negotiate protocol version {} is unsupported",
                    request.protocol_version
                ),
                None,
            )
            .await;
        }

        if !peer_uid_matches_current(&stream) {
            return reject(
                &mut stream,
                request.request_id,
                RejectCode::PermissionDenied,
                "peer uid is not authorized for this socket",
                None,
            )
            .await;
        }

        match request.upgrade {
            Upgrade::Proxy { service, mode } => {
                let target = self.resolve_proxy_target(&service).await;
                let Some(target) = target else {
                    return reject(
                        &mut stream,
                        request.request_id,
                        RejectCode::UnsupportedService,
                        format!("service '{service}' is not registered"),
                        None,
                    )
                    .await;
                };

                match target {
                    ServiceTarget::VsockPort(port) => {
                        match self.machine.connect_vsock(port).await {
                            Ok(vsock_stream) => {
                                accept(&mut stream, request.request_id, None).await?;
                                spawn_tunnel(stream, vsock_stream);
                                Ok(())
                            }
                            Err(err) => {
                                tracing::info!(error = %err, service = %service, "proxy service still starting");
                                reject(
                                    &mut stream,
                                    request.request_id,
                                    RejectCode::ServiceStarting,
                                    "service is starting",
                                    Some(RETRY_AFTER_STARTING_MS),
                                )
                                .await
                            }
                        }
                    }
                    ServiceTarget::Serial => {
                        let access = match mode {
                            ProxyMode::ReadOnly => SerialAccess::Watch,
                            ProxyMode::ReadWrite => SerialAccess::Interactive,
                        };

                        accept(&mut stream, request.request_id, None).await?;
                        let serial_stream = self.serial_console.open_stream(access).await?;
                        spawn_serial_tunnel(stream, serial_stream);
                        Ok(())
                    }
                }
            }
            Upgrade::VmMonitor { .. } => {
                accept(&mut stream, request.request_id, None).await?;
                services::serve(stream, self.store.clone()).await
            }
        }
    }

    async fn resolve_proxy_target(&self, service: &str) -> Option<ServiceTarget> {
        if service == ENDPOINT_SERIAL {
            return Some(ServiceTarget::Serial);
        }

        ServiceRegistry::discover(&self.machine)
            .await
            .ok()
            .and_then(|registry| registry.resolve(service))
    }
}

async fn accept(
    stream: &mut UnixStream,
    request_id: u64,
    message: Option<String>,
) -> eyre::Result<()> {
    Response::Accept(Accept {
        request_id,
        message,
    })
    .write_to(stream)
    .await
    .context("write Negotiate accept")?;
    Ok(())
}

async fn reject(
    stream: &mut UnixStream,
    request_id: u64,
    code: RejectCode,
    message: impl Into<String>,
    retry_after_ms: Option<u32>,
) -> eyre::Result<()> {
    Response::Reject(Reject {
        request_id,
        code,
        message: message.into(),
        retry_after_ms,
    })
    .write_to(stream)
    .await
    .context("write Negotiate reject")?;
    Ok(())
}

fn peer_uid_matches_current(stream: &UnixStream) -> bool {
    match peer_uid(stream) {
        Ok(peer_uid) => peer_uid == unsafe { libc::geteuid() },
        Err(err) => {
            tracing::warn!(error = %err, "failed to resolve peer uid");
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;

    let fd = stream.as_raw_fd();
    let mut euid: libc::uid_t = 0;
    let mut egid: libc::gid_t = 0;
    let rc = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(euid)
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;

    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast::<libc::c_void>(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(cred.uid)
}
