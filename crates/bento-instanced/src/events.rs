use eyre::Context;
use serde::{Deserialize, Serialize};
use std::io;
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InstancedEventType {
    Running,
    Exiting,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstancedEvent {
    pub timestamp: String,

    #[serde(rename = "type")]
    pub event_type: InstancedEventType,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub fn emit_event(event: &InstancedEvent) -> eyre::Result<()> {
    let mut out = io::stdout().lock();
    let mut data = serde_json::to_vec(event).context("serialize instanced event")?;
    data.push(b'\n');
    out.write_all(&data).context("write instanced event")?;
    out.flush().context("flush instanced event")?;
    Ok(())
}
