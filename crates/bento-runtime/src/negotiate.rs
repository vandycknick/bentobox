use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const NEGOTIATE_PROTOCOL_VERSION: u16 = 1;
pub const MAX_NEGOTIATE_FRAME_BYTES: usize = 16 * 1024;
pub const MAX_SERVICE_NAME_BYTES: usize = 256;
pub const MAX_MESSAGE_BYTES: usize = 1024;
pub const MAX_AUTH_TOKEN_BYTES: usize = 4096;
pub const NEGOTIATE_STREAM_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Upgrade {
    Proxy { service: String, mode: ProxyMode },
    InstanceControl { api_version: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Negotiate {
    pub protocol_version: u16,
    pub request_id: u64,
    pub upgrade: Upgrade,
    pub auth_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accept {
    pub request_id: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    UnsupportedProtocol,
    UnsupportedUpgrade,
    UnsupportedService,
    ServiceStarting,
    ServiceUnavailable,
    PermissionDenied,
    AuthFailed,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reject {
    pub request_id: u64,
    pub code: RejectCode,
    pub message: String,
    pub retry_after_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiateResult {
    Accept(Accept),
    Reject(Reject),
}

#[derive(Debug)]
pub enum ClientUpgradeStreamError {
    Io(io::Error),
    Reject(Reject),
}

impl std::fmt::Display for ClientUpgradeStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Reject(reject) => write!(f, "{}", reject.message),
        }
    }
}

impl std::error::Error for ClientUpgradeStreamError {}

impl Negotiate {
    pub fn new(request_id: u64, upgrade: Upgrade) -> Self {
        Self {
            protocol_version: NEGOTIATE_PROTOCOL_VERSION,
            request_id,
            upgrade,
            auth_token: None,
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        if let Upgrade::Proxy { service, .. } = &self.upgrade {
            if service.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "proxy service cannot be empty",
                ));
            }

            if service.len() > MAX_SERVICE_NAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "proxy service exceeded max length",
                ));
            }
        }

        if let Some(token) = &self.auth_token {
            if token.len() > MAX_AUTH_TOKEN_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "auth token exceeded max length",
                ));
            }
        }

        Ok(())
    }

    pub fn read_from(stream: &mut impl Read) -> io::Result<Self> {
        let payload = read_frame(stream)?;
        let msg = deserialize_frame::<Self>(&payload)?;
        msg.validate()?;
        Ok(msg)
    }

    pub fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        self.validate()?;
        write_framed(stream, self)
    }

    pub async fn read_from_async(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Self> {
        let payload = read_frame_async(stream).await?;
        let msg = deserialize_frame::<Self>(&payload)?;
        msg.validate()?;
        Ok(msg)
    }

    pub async fn write_to_async(&self, stream: &mut (impl AsyncWrite + Unpin)) -> io::Result<()> {
        self.validate()?;
        write_framed_async(stream, self).await
    }

    pub fn client_upgrade_stream_v1(
        stream: &mut UnixStream,
        upgrade: Upgrade,
    ) -> Result<(), ClientUpgradeStreamError> {
        stream
            .set_read_timeout(Some(NEGOTIATE_STREAM_TIMEOUT))
            .map_err(ClientUpgradeStreamError::Io)?;
        stream
            .set_write_timeout(Some(NEGOTIATE_STREAM_TIMEOUT))
            .map_err(ClientUpgradeStreamError::Io)?;

        Negotiate::new(1, upgrade)
            .write_to(stream)
            .map_err(ClientUpgradeStreamError::Io)?;

        match NegotiateResult::read_from(stream).map_err(ClientUpgradeStreamError::Io)? {
            NegotiateResult::Accept(_) => {
                stream
                    .set_read_timeout(None)
                    .map_err(ClientUpgradeStreamError::Io)?;
                stream
                    .set_write_timeout(None)
                    .map_err(ClientUpgradeStreamError::Io)?;
                Ok(())
            }
            NegotiateResult::Reject(reject) => Err(ClientUpgradeStreamError::Reject(reject)),
        }
    }

    pub async fn client_upgrade_stream_v1_async(
        stream: tokio::net::UnixStream,
        upgrade: Upgrade,
    ) -> Result<tokio::net::UnixStream, ClientUpgradeStreamError> {
        let mut stream = stream;
        let negotiate = async {
            Negotiate::new(1, upgrade)
                .write_to_async(&mut stream)
                .await
                .map_err(ClientUpgradeStreamError::Io)?;

            match NegotiateResult::read_from_async(&mut stream)
                .await
                .map_err(ClientUpgradeStreamError::Io)?
            {
                NegotiateResult::Accept(_) => Ok(()),
                NegotiateResult::Reject(reject) => Err(ClientUpgradeStreamError::Reject(reject)),
            }
        };

        match tokio::time::timeout(NEGOTIATE_STREAM_TIMEOUT, negotiate).await {
            Ok(Ok(())) => Ok(stream),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(ClientUpgradeStreamError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "negotiate stream timed out",
            ))),
        }
    }
}

impl Accept {
    pub fn validate(&self) -> io::Result<()> {
        if let Some(message) = &self.message {
            if message.len() > MAX_MESSAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "accept message exceeded max length",
                ));
            }
        }

        Ok(())
    }

    pub fn read_from(stream: &mut impl Read) -> io::Result<Self> {
        let payload = read_frame(stream)?;
        let msg = deserialize_frame::<Self>(&payload)?;
        msg.validate()?;
        Ok(msg)
    }

    pub fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        self.validate()?;
        write_framed(stream, self)
    }

    pub async fn read_from_async(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Self> {
        let payload = read_frame_async(stream).await?;
        let msg = deserialize_frame::<Self>(&payload)?;
        msg.validate()?;
        Ok(msg)
    }

    pub async fn write_to_async(&self, stream: &mut (impl AsyncWrite + Unpin)) -> io::Result<()> {
        self.validate()?;
        write_framed_async(stream, self).await
    }
}

impl Reject {
    pub fn validate(&self) -> io::Result<()> {
        if self.message.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "reject message cannot be empty",
            ));
        }

        if self.message.len() > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "reject message exceeded max length",
            ));
        }

        Ok(())
    }

    pub fn read_from(stream: &mut impl Read) -> io::Result<Self> {
        let payload = read_frame(stream)?;
        let msg = deserialize_frame::<Self>(&payload)?;
        msg.validate()?;
        Ok(msg)
    }

    pub fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        self.validate()?;
        write_framed(stream, self)
    }

    pub async fn read_from_async(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Self> {
        let payload = read_frame_async(stream).await?;
        let msg = deserialize_frame::<Self>(&payload)?;
        msg.validate()?;
        Ok(msg)
    }

    pub async fn write_to_async(&self, stream: &mut (impl AsyncWrite + Unpin)) -> io::Result<()> {
        self.validate()?;
        write_framed_async(stream, self).await
    }
}

impl NegotiateResult {
    pub fn read_from(stream: &mut impl Read) -> io::Result<Self> {
        let payload = read_frame(stream)?;
        decode_result(&payload)
    }

    pub async fn read_from_async(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Self> {
        let payload = read_frame_async(stream).await?;
        decode_result(&payload)
    }
}

fn write_framed(stream: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let payload = postcard::to_stdvec(value).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("serialize Negotiate frame: {err}"),
        )
    })?;

    if payload.len() > MAX_NEGOTIATE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Negotiate frame exceeded max size",
        ));
    }

    let len = payload.len() as u32;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()
}

async fn write_framed_async(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
) -> io::Result<()> {
    let payload = postcard::to_stdvec(value).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("serialize Negotiate frame: {err}"),
        )
    })?;

    if payload.len() > MAX_NEGOTIATE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Negotiate frame exceeded max size",
        ));
    }

    let len = payload.len() as u32;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(&payload).await?;
    stream.flush().await
}

fn read_frame(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let len = u32::from_le_bytes(header) as usize;

    if len > MAX_NEGOTIATE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Negotiate frame exceeded max size",
        ));
    }

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

async fn read_frame_async(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let len = u32::from_le_bytes(header) as usize;

    if len > MAX_NEGOTIATE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Negotiate frame exceeded max size",
        ));
    }

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

fn deserialize_frame<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> io::Result<T> {
    postcard::from_bytes(payload).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decode Negotiate frame failed: {err}"),
        )
    })
}

fn decode_result(payload: &[u8]) -> io::Result<NegotiateResult> {
    if let Ok(accept) = postcard::from_bytes::<Accept>(payload) {
        accept.validate()?;
        return Ok(NegotiateResult::Accept(accept));
    }

    if let Ok(reject) = postcard::from_bytes::<Reject>(payload) {
        reject.validate()?;
        return Ok(NegotiateResult::Reject(reject));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "decode Negotiate result frame failed",
    ))
}
