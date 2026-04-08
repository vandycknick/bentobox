use serde::{Deserialize, Serialize};

/// Well-known service endpoint names used by the Negotiate protocol.
pub const ENDPOINT_SSH: &str = "ssh";
pub const ENDPOINT_SERIAL: &str = "serial";
pub const ENDPOINT_DOCKER: &str = "docker";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceDescriptor {
    pub name: String,
}
