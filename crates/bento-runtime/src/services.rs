use serde::{Deserialize, Serialize};

pub const SERVICE_SSH: &str = "ssh";
pub const SERVICE_SERIAL: &str = "serial";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceDescriptor {
    pub name: String,
}
