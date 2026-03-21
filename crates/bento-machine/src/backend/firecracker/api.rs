use std::path::PathBuf;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::Method;
use serde::Serialize;

use crate::types::MachineError;

pub(super) struct FirecrackerApiClient {
    socket_path: PathBuf,
    client: Client,
}

impl FirecrackerApiClient {
    pub(super) fn new(socket_path: PathBuf, timeout: Duration) -> Result<Self, MachineError> {
        let client = Client::builder()
            .http1_only()
            .connect_timeout(timeout)
            .timeout(timeout)
            .unix_socket(socket_path.clone())
            .build()
            .map_err(|err| {
                MachineError::Backend(format!(
                    "build firecracker reqwest client failed for {}: {err}",
                    socket_path.display()
                ))
            })?;
        Ok(Self {
            socket_path,
            client,
        })
    }

    pub(super) fn put_json<T: Serialize>(&self, path: &str, body: &T) -> Result<(), MachineError> {
        self.send_request(Method::PUT, path, Some(body))
    }

    pub(super) fn ping(&self) -> Result<(), MachineError> {
        self.send_request::<()>(Method::GET, "/", None)
    }

    fn send_request<T: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&T>,
    ) -> Result<(), MachineError> {
        tracing::debug!(method = %method, path, api_socket = %self.socket_path.display(), "sending firecracker API request");
        let url = format!("http://localhost{path}");
        let request = self
            .client
            .request(method.clone(), &url)
            .header("Accept", "application/json");

        let request = if let Some(body) = body {
            request
                .header("Content-Type", "application/json")
                .json(body)
        } else {
            request
        };

        let response = request.send().map_err(|err| {
            MachineError::Backend(format!(
                "firecracker API request failed for {} {}: {err}",
                method, path
            ))
        })?;

        let status = response.status();
        let response_text = response.text().map_err(|err| {
            MachineError::Backend(format!(
                "read firecracker API response failed for {} {}: {err}",
                method, path
            ))
        })?;

        if status.is_success() {
            tracing::debug!(method = %method, path, status_code = status.as_u16(), "firecracker API request succeeded");
            return Ok(());
        }

        tracing::warn!(
            method = %method,
            path,
            status_code = status.as_u16(),
            response_body = response_text.trim(),
            "firecracker API request failed"
        );
        Err(MachineError::Backend(format!(
            "firecracker API request {} {} failed with status {}: {}",
            method,
            path,
            status,
            response_text.trim()
        )))
    }
}

#[derive(Serialize)]
pub(super) struct MachineConfigurationRequest {
    pub(super) vcpu_count: usize,
    pub(super) mem_size_mib: u64,
    pub(super) smt: bool,
    pub(super) track_dirty_pages: bool,
}

#[derive(Serialize)]
pub(super) struct BootSourceRequest {
    pub(super) kernel_image_path: String,
    pub(super) initrd_path: String,
    pub(super) boot_args: String,
}

#[derive(Serialize)]
pub(super) struct ActionRequest {
    pub(super) action_type: &'static str,
}

#[derive(Serialize)]
pub(super) struct DriveRequest {
    pub(super) drive_id: String,
    pub(super) partuuid: Option<String>,
    pub(super) is_root_device: bool,
    pub(super) cache_type: &'static str,
    pub(super) is_read_only: bool,
    pub(super) path_on_host: String,
    pub(super) io_engine: &'static str,
}

#[derive(Serialize)]
pub(super) struct VsockRequest {
    pub(super) guest_cid: u32,
    pub(super) uds_path: String,
}
