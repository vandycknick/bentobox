use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CONTROL_PROTOCOL_VERSION: u16 = 1;
const MAX_CONTROL_LINE_BYTES: usize = 16 * 1024;
pub const SERVICE_SSH: &str = "ssh";
pub const SERVICE_SERIAL: &str = "serial";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRequest {
    pub version: u16,
    pub id: String,
    #[serde(flatten)]
    pub body: ControlRequestBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ControlRequestBody {
    OpenService {
        service: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        options: Map<String, Value>,
    },
    ListServices,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlResponse {
    pub version: u16,
    pub id: String,
    #[serde(flatten)]
    pub body: ControlResponseBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControlResponseBody {
    Opened,
    Starting {
        attempt: u8,
        max_attempts: u8,
        retry_after_secs: u64,
    },
    Services {
        services: Vec<ServiceDescriptor>,
    },
    Error {
        code: ControlErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceDescriptor {
    pub name: String,
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

impl ControlRequest {
    pub fn v1_open_service(id: impl Into<String>, service: impl Into<String>) -> Self {
        Self::v1_open_service_with_options(id, service, Map::new())
    }

    pub fn v1_open_service_with_options(
        id: impl Into<String>,
        service: impl Into<String>,
        options: Map<String, Value>,
    ) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id: id.into(),
            body: ControlRequestBody::OpenService {
                service: service.into(),
                options,
            },
        }
    }

    pub fn v1_list_services(id: impl Into<String>) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id: id.into(),
            body: ControlRequestBody::ListServices,
        }
    }

    pub fn read_from(stream: &mut impl Read) -> io::Result<Self> {
        let line = read_json_line(stream)?;
        if line.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "control request stream closed before request",
            ));
        }

        serde_json::from_str::<Self>(&line).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse control request json: {err}"),
            )
        })
    }

    pub fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        write_json_line(stream, self)
    }
}

impl ControlResponse {
    pub fn v1_opened(id: impl Into<String>) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id: id.into(),
            body: ControlResponseBody::Opened,
        }
    }

    pub fn v1_services(id: impl Into<String>, services: Vec<ServiceDescriptor>) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id: id.into(),
            body: ControlResponseBody::Services { services },
        }
    }

    pub fn v1_starting(
        id: impl Into<String>,
        attempt: u8,
        max_attempts: u8,
        retry_after_secs: u64,
    ) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id: id.into(),
            body: ControlResponseBody::Starting {
                attempt,
                max_attempts,
                retry_after_secs,
            },
        }
    }

    pub fn v1_error(
        id: impl Into<String>,
        code: ControlErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            id: id.into(),
            body: ControlResponseBody::Error {
                code,
                message: message.into(),
            },
        }
    }

    pub fn read_from(stream: &mut impl Read) -> io::Result<Self> {
        let line = read_json_line(stream)?;
        if line.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "control response stream closed before response",
            ));
        }

        serde_json::from_str::<Self>(&line).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse control response json: {err}"),
            )
        })
    }

    pub fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        write_json_line(stream, self)
    }
}

pub fn read_json_line(stream: &mut impl Read) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let n = stream.read(&mut byte)?;
        if n == 0 {
            break;
        }

        if byte[0] == b'\n' {
            break;
        }

        buf.push(byte[0]);
        if buf.len() > MAX_CONTROL_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control line exceeded 16KiB",
            ));
        }
    }

    String::from_utf8(buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control line was not utf-8"))
}

pub fn write_json_line<T: Serialize>(stream: &mut impl Write, value: &T) -> io::Result<()> {
    let mut payload = serde_json::to_vec(value).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("serialize control message: {err}"),
        )
    })?;
    payload.push(b'\n');
    stream.write_all(&payload)?;
    stream.flush()
}
