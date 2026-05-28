use std::error::Error;
use std::str::FromStr;

use bento_core::MachineId;
use sqlx::sqlite::SqliteRow;
use sqlx::{FromRow, Row};

use crate::models::{
    Machine, NetworkAttachment, NetworkDefinition, NetworkDriverPreference, NetworkInstance,
};

pub(crate) struct DbMachine(pub(crate) Machine);
pub(crate) struct DbNetworkAttachment(pub(crate) NetworkAttachment);
pub(crate) struct DbNetworkInstance(pub(crate) NetworkInstance);
pub(crate) struct DbNetworkDefinition(pub(crate) NetworkDefinition);

impl<'row> FromRow<'row, SqliteRow> for DbMachine {
    fn from_row(row: &'row SqliteRow) -> sqlx::Result<Self> {
        let id_str: String = row.try_get("id")?;
        let id = parse_machine_id(&id_str, "machines.id")?;
        Ok(Self(Machine {
            id,
            name: row.try_get("name")?,
            instance_dir: row.try_get("instance_dir")?,
            created_at: row.try_get("created_at")?,
            modified_at: row.try_get("modified_at")?,
            image_ref: row.try_get("image_ref")?,
            labels: deserialize_json(row.try_get("labels")?, "machines.labels")?,
            metadata: deserialize_json(row.try_get("metadata")?, "machines.metadata")?,
            network: deserialize_json(row.try_get("network")?, "machines.network")?,
        }))
    }
}

impl<'row> FromRow<'row, SqliteRow> for DbNetworkAttachment {
    fn from_row(row: &'row SqliteRow) -> sqlx::Result<Self> {
        let id_str: String = row.try_get("machine_id")?;
        let machine_id = parse_machine_id(&id_str, "network_attachments.machine_id")?;
        Ok(Self(NetworkAttachment {
            machine_id,
            network_instance_id: row.try_get("network_instance_id")?,
            guest_mac: row.try_get("guest_mac")?,
            created_at: row.try_get("created_at")?,
            modified_at: row.try_get("modified_at")?,
        }))
    }
}

impl<'row> FromRow<'row, SqliteRow> for DbNetworkInstance {
    fn from_row(row: &'row SqliteRow) -> sqlx::Result<Self> {
        Ok(Self(NetworkInstance {
            id: row.try_get("id")?,
            driver: row.try_get("driver")?,
            definition_name: row.try_get("definition_name")?,
            runtime_dir: row.try_get("runtime_dir")?,
            attachment_json: row.try_get("attachment_json")?,
            driver_state_json: row.try_get("driver_state_json")?,
            state: row.try_get("state")?,
            created_at: row.try_get("created_at")?,
            modified_at: row.try_get("modified_at")?,
        }))
    }
}

impl<'row> FromRow<'row, SqliteRow> for DbNetworkDefinition {
    fn from_row(row: &'row SqliteRow) -> sqlx::Result<Self> {
        let name: String = row.try_get("name")?;
        let mode = deserialize_json(row.try_get("mode")?, "network_definitions.mode")?;
        let driver_preference: NetworkDriverPreference = deserialize_json(
            row.try_get("driver_preference")?,
            "network_definitions.driver_preference",
        )?;
        Ok(Self(NetworkDefinition {
            name,
            mode,
            driver_preference,
        }))
    }
}

fn parse_machine_id(value: &str, field: &'static str) -> sqlx::Result<MachineId> {
    MachineId::from_str(value).map_err(|err| column_decode_error(field, err))
}

fn deserialize_json<T>(value: String, field: &'static str) -> sqlx::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(&value).map_err(|err| column_decode_error(field, err))
}

fn column_decode_error(
    field: &'static str,
    source: impl Error + Send + Sync + 'static,
) -> sqlx::Error {
    sqlx::Error::ColumnDecode {
        index: field.to_string(),
        source: Box::new(source),
    }
}
