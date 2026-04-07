use std::path::Path;
use std::time::Duration;

use bento_core::MachineId;
use rusqlite::{params, Connection, OptionalExtension};

use crate::{Layout, LibVmError};

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineMetadata {
    pub id: MachineId,
    pub name: String,
    pub instance_dir: String,
}

pub struct StateStore {
    conn: Connection,
}

impl StateStore {
    pub fn open(layout: &Layout) -> Result<Self, LibVmError> {
        std::fs::create_dir_all(layout.data_dir())?;
        let conn = open_connection(&layout.state_db_path())?;
        run_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn insert_machine(&self, metadata: &MachineMetadata) -> Result<(), LibVmError> {
        self.conn.execute(
            "INSERT INTO machines (id, name, instance_dir) VALUES (?1, ?2, ?3)",
            params![
                metadata.id.to_string(),
                metadata.name,
                metadata.instance_dir
            ],
        )?;
        Ok(())
    }

    pub fn get_machine_by_id(&self, id: MachineId) -> Result<Option<MachineMetadata>, LibVmError> {
        self.conn
            .query_row(
                "SELECT id, name, instance_dir FROM machines WHERE id = ?1",
                params![id.to_string()],
                row_to_metadata,
            )
            .optional()
            .map_err(LibVmError::from)
    }

    pub fn get_machine_by_name(&self, name: &str) -> Result<Option<MachineMetadata>, LibVmError> {
        self.conn
            .query_row(
                "SELECT id, name, instance_dir FROM machines WHERE name = ?1",
                params![name],
                row_to_metadata,
            )
            .optional()
            .map_err(LibVmError::from)
    }

    pub fn get_machine_by_id_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<MachineMetadata>, LibVmError> {
        let pattern = format!("{prefix}%");
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, instance_dir FROM machines WHERE id LIKE ?1")?;
        let rows = stmt.query_map(params![pattern], row_to_metadata)?;
        let mut machines = Vec::new();
        for row in rows {
            machines.push(row?);
        }
        Ok(machines)
    }

    pub fn list_machines(&self) -> Result<Vec<MachineMetadata>, LibVmError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, instance_dir FROM machines ORDER BY name")?;
        let rows = stmt.query_map([], row_to_metadata)?;
        let mut machines = Vec::new();
        for row in rows {
            machines.push(row?);
        }
        Ok(machines)
    }

    pub fn remove_machine(&self, metadata: &MachineMetadata) -> Result<(), LibVmError> {
        self.conn.execute(
            "DELETE FROM machines WHERE id = ?1",
            params![metadata.id.to_string()],
        )?;
        Ok(())
    }
}

fn open_connection(path: &Path) -> Result<Connection, LibVmError> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

fn run_migrations(conn: &Connection) -> Result<(), LibVmError> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS machines (
                id           TEXT PRIMARY KEY,
                name         TEXT NOT NULL UNIQUE,
                instance_dir TEXT NOT NULL
            );
            PRAGMA user_version = 1;",
        )?;
    }

    debug_assert_eq!(
        conn.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
            .unwrap_or(0),
        SCHEMA_VERSION,
        "schema version mismatch after migration"
    );

    Ok(())
}

fn row_to_metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<MachineMetadata> {
    let id_str: String = row.get(0)?;
    let id: MachineId = id_str.parse().map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(MachineMetadata {
        id,
        name: row.get(1)?,
        instance_dir: row.get(2)?,
    })
}

pub fn metadata_from_path(id: MachineId, name: String, instance_dir: &Path) -> MachineMetadata {
    MachineMetadata {
        id,
        name,
        instance_dir: instance_dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use bento_core::MachineId;

    use super::{metadata_from_path, StateStore};
    use crate::Layout;

    fn temp_layout() -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let layout = Layout::new(dir.path());
        (dir, layout)
    }

    #[test]
    fn insert_and_lookup_by_name() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");
        let id = MachineId::new();
        let metadata = metadata_from_path(id, "devbox".to_string(), &layout.instance_dir(id));

        store.insert_machine(&metadata).expect("insert");
        let found = store
            .get_machine_by_name("devbox")
            .expect("lookup")
            .expect("should find machine");

        assert_eq!(found, metadata);
    }

    #[test]
    fn insert_and_lookup_by_id() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");
        let id = MachineId::new();
        let metadata = metadata_from_path(id, "testvm".to_string(), &layout.instance_dir(id));

        store.insert_machine(&metadata).expect("insert");
        let found = store
            .get_machine_by_id(id)
            .expect("lookup")
            .expect("should find machine");

        assert_eq!(found, metadata);
    }

    #[test]
    fn lookup_by_id_prefix() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");
        let id = MachineId::new();
        let metadata = metadata_from_path(id, "prefix-test".to_string(), &layout.instance_dir(id));

        store.insert_machine(&metadata).expect("insert");

        let id_str = id.to_string();
        let prefix = &id_str[..8];
        let found = store.get_machine_by_id_prefix(prefix).expect("lookup");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0], metadata);
    }

    #[test]
    fn list_machines_sorted_by_name() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");

        let id_b = MachineId::new();
        let id_a = MachineId::new();
        store
            .insert_machine(&metadata_from_path(
                id_b,
                "bravo".to_string(),
                &layout.instance_dir(id_b),
            ))
            .expect("insert b");
        store
            .insert_machine(&metadata_from_path(
                id_a,
                "alpha".to_string(),
                &layout.instance_dir(id_a),
            ))
            .expect("insert a");

        let list = store.list_machines().expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "bravo");
    }

    #[test]
    fn remove_machine() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");
        let id = MachineId::new();
        let metadata = metadata_from_path(id, "gonner".to_string(), &layout.instance_dir(id));

        store.insert_machine(&metadata).expect("insert");
        store.remove_machine(&metadata).expect("remove");

        let found = store.get_machine_by_id(id).expect("lookup");
        assert!(found.is_none());
    }

    #[test]
    fn duplicate_name_fails() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");

        let id1 = MachineId::new();
        let id2 = MachineId::new();
        store
            .insert_machine(&metadata_from_path(
                id1,
                "dupe".to_string(),
                &layout.instance_dir(id1),
            ))
            .expect("insert first");

        let result = store.insert_machine(&metadata_from_path(
            id2,
            "dupe".to_string(),
            &layout.instance_dir(id2),
        ));
        assert!(result.is_err(), "duplicate name should fail");
    }

    #[test]
    fn concurrent_connections_work() {
        let (_dir, layout) = temp_layout();
        let store1 = StateStore::open(&layout).expect("open store 1");
        let store2 = StateStore::open(&layout).expect("open store 2");

        let id = MachineId::new();
        store1
            .insert_machine(&metadata_from_path(
                id,
                "shared".to_string(),
                &layout.instance_dir(id),
            ))
            .expect("insert via store1");

        let found = store2
            .get_machine_by_name("shared")
            .expect("lookup via store2")
            .expect("should find machine from other connection");

        assert_eq!(found.id, id);
    }
}
