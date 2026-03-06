use bento_machine::{MachineHandle, OpenDeviceRequest, OpenDeviceResponse};
use bento_runtime::negotiate::{
    Accept, Negotiate, ProxyMode, Reject, RejectCode, Response, Upgrade, NEGOTIATE_PROTOCOL_VERSION,
};
use bento_runtime::services::SERVICE_SERIAL;
use eyre::Context;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixStream;

use crate::discovery::{ServiceRegistry, ServiceTarget};
use crate::instance_control_service;
use crate::serial::{spawn_serial_tunnel, SerialAccess, SerialRuntime};
use crate::state::InstanceStore;
use crate::tunnel::spawn_tunnel;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_AFTER_STARTING_MS: u32 = 1000;

pub(crate) async fn handle_client(
    mut stream: UnixStream,
    machine: MachineHandle,
    serial_runtime: Arc<SerialRuntime>,
    store: Arc<InstanceStore>,
) -> eyre::Result<()> {
    let request =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, Negotiate::read_from(&mut stream)).await {
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
            let target = resolve_proxy_target(&machine, &service).await;
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
                    match machine.open_device(OpenDeviceRequest::Vsock { port }).await {
                        Ok(OpenDeviceResponse::Vsock { stream: vsock_fd }) => {
                            accept(&mut stream, request.request_id, None).await?;
                            spawn_tunnel(stream, vsock_fd);
                            Ok(())
                        }
                        Ok(_) => {
                            reject(
                                &mut stream,
                                request.request_id,
                                RejectCode::Internal,
                                "driver returned unexpected device type for proxy service",
                                None,
                            )
                            .await
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
                    spawn_serial_tunnel(stream, serial_runtime, access);
                    Ok(())
                }
            }
        }
        Upgrade::InstanceControl { .. } => {
            accept(&mut stream, request.request_id, None).await?;
            instance_control_service::serve(stream, store).await
        }
    }
}

async fn resolve_proxy_target(machine: &MachineHandle, service: &str) -> Option<ServiceTarget> {
    if service == SERVICE_SERIAL {
        return Some(ServiceTarget::Serial);
    }

    ServiceRegistry::discover(machine)
        .await
        .ok()
        .and_then(|registry| registry.resolve(service))
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
