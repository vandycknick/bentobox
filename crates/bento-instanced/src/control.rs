use bento_runtime::driver::{Driver, OpenDeviceRequest, OpenDeviceResponse};
use bento_runtime::instance_control::{
    ControlErrorCode, ControlRequest, ControlRequestBody, ControlResponse,
    CONTROL_PROTOCOL_VERSION, SERVICE_SERIAL,
};
use eyre::Context;
use serde_json::{Map, Value};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use crate::discovery::{ServiceRegistry, ServiceTarget};
use crate::serial::{parse_serial_open_options, spawn_serial_tunnel, SerialRuntime};
use crate::tunnel::spawn_tunnel;

const SERVICE_OPEN_MAX_ATTEMPTS: u8 = 5;
const SERVICE_OPEN_RETRY_DELAY_SECS: u64 = 2;

pub(crate) async fn handle_client(
    mut stream: UnixStream,
    driver: &dyn Driver,
    serial_runtime: Arc<SerialRuntime>,
) -> eyre::Result<()> {
    let request = ControlRequest::read_from(&mut stream).context("read control request")?;

    if request.version != CONTROL_PROTOCOL_VERSION {
        let response = ControlResponse::v1_error(
            request.id,
            ControlErrorCode::UnsupportedVersion,
            format!(
                "control protocol version {} is unsupported",
                request.version
            ),
        );
        response
            .write_to(&mut stream)
            .context("write control response")?;
        return Ok(());
    }

    match request.body {
        ControlRequestBody::ListServices => {
            let service_registry = match ServiceRegistry::discover(driver).await {
                Ok(service_registry) => service_registry,
                Err(err) => {
                    ControlResponse::v1_error(
                        request.id,
                        ControlErrorCode::ServiceUnavailable,
                        format!("discover guest services failed: {err}"),
                    )
                    .write_to(&mut stream)
                    .context("write control response")?;
                    return Ok(());
                }
            };
            let response = ControlResponse::v1_services(request.id, service_registry.describe());
            response
                .write_to(&mut stream)
                .context("write control response")?;
        }
        ControlRequestBody::OpenService { service, options } => {
            let id = request.id;
            let target = if service == SERVICE_SERIAL {
                Some(ServiceTarget::Serial)
            } else {
                let service_registry = match ServiceRegistry::discover(driver).await {
                    Ok(service_registry) => service_registry,
                    Err(err) => {
                        ControlResponse::v1_error(
                            id,
                            ControlErrorCode::ServiceUnavailable,
                            format!("discover guest services failed: {err}"),
                        )
                        .write_to(&mut stream)
                        .context("write control response")?;
                        return Ok(());
                    }
                };
                service_registry.resolve(&service)
            };

            let Some(target) = target else {
                let response = ControlResponse::v1_error(
                    id,
                    ControlErrorCode::UnknownService,
                    format!("service '{service}' is not registered"),
                );
                response
                    .write_to(&mut stream)
                    .context("write control response")?;
                return Ok(());
            };

            match target {
                ServiceTarget::VsockPort(port) => {
                    if !options.is_empty() {
                        ControlResponse::v1_error(
                            id,
                            ControlErrorCode::UnsupportedRequest,
                            "ssh service does not accept options",
                        )
                        .write_to(&mut stream)
                        .context("write control response")?;
                        return Ok(());
                    }

                    let mut last_err = None;

                    for attempt in 1..=SERVICE_OPEN_MAX_ATTEMPTS {
                        match driver.open_device(OpenDeviceRequest::Vsock { port }) {
                            Ok(OpenDeviceResponse::Vsock { stream: vsock_fd }) => {
                                ControlResponse::v1_opened(id.clone())
                                    .write_to(&mut stream)
                                    .context("write control response")?;
                                spawn_tunnel(stream, vsock_fd);
                                return Ok(());
                            }
                            Ok(_) => {
                                last_err = Some(eyre::eyre!(
                                    "driver returned unexpected device type for ssh service"
                                ));
                            }
                            Err(err) => {
                                last_err = Some(eyre::eyre!(err));

                                if attempt < SERVICE_OPEN_MAX_ATTEMPTS {
                                    ControlResponse::v1_starting(
                                        id.clone(),
                                        attempt,
                                        SERVICE_OPEN_MAX_ATTEMPTS,
                                        SERVICE_OPEN_RETRY_DELAY_SECS,
                                    )
                                    .write_to(&mut stream)
                                    .context("write control response")?;

                                    tokio::time::sleep(Duration::from_secs(
                                        SERVICE_OPEN_RETRY_DELAY_SECS,
                                    ))
                                    .await;
                                }
                            }
                        }
                    }

                    let err_text = last_err
                        .map(|err| err.to_string())
                        .unwrap_or_else(|| "unknown startup error".to_string());

                    ControlResponse::v1_error(
                        id,
                        ControlErrorCode::ServiceUnavailable,
                        format!(
                            "failed to open service '{service}' on vsock port {port} after {} attempts: {err_text}",
                            SERVICE_OPEN_MAX_ATTEMPTS
                        ),
                    )
                    .write_to(&mut stream)
                    .context("write control response")?;
                }
                ServiceTarget::Serial => {
                    handle_open_serial(stream, id, options, serial_runtime)?;
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn handle_open_serial(
    mut stream: UnixStream,
    id: String,
    options: Map<String, Value>,
    serial_runtime: Arc<SerialRuntime>,
) -> eyre::Result<()> {
    let access = match parse_serial_open_options(options) {
        Ok(access) => access,
        Err(message) => {
            ControlResponse::v1_error(id, ControlErrorCode::UnsupportedRequest, message)
                .write_to(&mut stream)
                .context("write control response")?;
            return Ok(());
        }
    };

    ControlResponse::v1_opened(id)
        .write_to(&mut stream)
        .context("write control response")?;
    spawn_serial_tunnel(stream, serial_runtime, access);

    Ok(())
}
