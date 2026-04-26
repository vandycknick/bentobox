use std::path::Path;
use std::time::Duration;

use crate::api;
use crate::error::FirecrackerError;

pub(crate) const DEFAULT_BASE_URL: &str = "http://localhost";
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn api_client(
    socket_path: &Path,
    timeout: Duration,
) -> Result<api::Client, FirecrackerError> {
    let client = reqwest::Client::builder()
        .http1_only()
        .connect_timeout(timeout)
        .timeout(timeout)
        .unix_socket(socket_path)
        .build()
        .map_err(FirecrackerError::HttpClient)?;

    Ok(api::Client::new_with_client(DEFAULT_BASE_URL, client))
}
