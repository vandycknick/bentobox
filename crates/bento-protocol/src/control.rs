use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const SERVICE_SSH: &str = "ssh";
pub const SERVICE_SERIAL: &str = "serial";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceDescriptor {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenServiceRequest {
    pub service: String,
    #[serde(default)]
    pub options: Map<String, Value>,
}

impl OpenServiceRequest {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            options: Map::new(),
        }
    }

    pub fn with_options(service: impl Into<String>, options: Map<String, Value>) -> Self {
        Self {
            service: service.into(),
            options,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenServiceResponse {
    pub socket_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    UnsupportedVersion,
    UnsupportedRequest,
    UnknownService,
    ServiceUnavailable,
    InstanceNotRunning,
    PermissionDenied,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: String,
}

impl ControlError {
    pub fn new(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tarpc::service]
pub trait ControlPlane {
    async fn list_services() -> Result<Vec<ServiceDescriptor>, ControlError>;
    async fn open_service(request: OpenServiceRequest)
        -> Result<OpenServiceResponse, ControlError>;
}
