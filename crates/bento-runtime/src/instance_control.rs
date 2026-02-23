use serde::{Deserialize, Serialize};

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const CONTROL_OP_OPEN_VSOCK: &str = "open_vsock";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRequest {
    pub version: u32,
    pub id: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlResponse {
    pub version: u32,
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ControlResponse {
    pub fn ok(id: String) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id,
            ok: true,
            code: None,
            message: None,
        }
    }

    pub fn error(id: String, code: &str, message: impl Into<String>) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id,
            ok: false,
            code: Some(code.to_string()),
            message: Some(message.into()),
        }
    }
}
