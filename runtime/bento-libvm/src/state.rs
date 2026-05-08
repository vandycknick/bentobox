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
    pub created_at: i64,
    pub modified_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInstanceState {
    pub id: String,
    pub driver: String,
    pub definition_name: Option<String>,
    pub subnet_cidr: String,
    pub runtime_dir: String,
    pub helper_pid: i32,
    pub transport_socket_path: String,
    pub log_path: String,
    pub pid_file_path: String,
    pub pcap_path: Option<String>,
    pub state: String,
    pub created_at: i64,
    pub modified_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAttachmentState {
    pub machine_id: MachineId,
    pub network_instance_id: String,
    pub guest_mac: String,
    pub created_at: i64,
    pub modified_at: i64,
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
            "INSERT INTO machines (id, name, instance_dir, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                metadata.id.to_string(),
                metadata.name,
                metadata.instance_dir,
                metadata.created_at,
                metadata.modified_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_machine_by_id(&self, id: MachineId) -> Result<Option<MachineMetadata>, LibVmError> {
        self.conn
            .query_row(
                "SELECT id, name, instance_dir, created_at, modified_at FROM machines WHERE id = ?1",
                params![id.to_string()],
                row_to_metadata,
            )
            .optional()
            .map_err(LibVmError::from)
    }

    pub fn get_machine_by_name(&self, name: &str) -> Result<Option<MachineMetadata>, LibVmError> {
        self.conn
            .query_row(
                "SELECT id, name, instance_dir, created_at, modified_at FROM machines WHERE name = ?1",
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
        let mut stmt = self.conn.prepare(
            "SELECT id, name, instance_dir, created_at, modified_at FROM machines WHERE id LIKE ?1",
        )?;
        let rows = stmt.query_map(params![pattern], row_to_metadata)?;
        let mut machines = Vec::new();
        for row in rows {
            machines.push(row?);
        }
        Ok(machines)
    }

    pub fn list_machines(&self) -> Result<Vec<MachineMetadata>, LibVmError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, instance_dir, created_at, modified_at FROM machines ORDER BY name",
        )?;
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

    pub fn get_network_attachment(
        &self,
        machine_id: MachineId,
    ) -> Result<Option<NetworkAttachmentState>, LibVmError> {
        self.conn
            .query_row(
                "SELECT machine_id, network_instance_id, guest_mac, created_at, modified_at
                 FROM network_attachments WHERE machine_id = ?1",
                params![machine_id.to_string()],
                row_to_network_attachment,
            )
            .optional()
            .map_err(LibVmError::from)
    }

    pub fn get_network_instance(
        &self,
        network_id: &str,
    ) -> Result<Option<NetworkInstanceState>, LibVmError> {
        self.conn
            .query_row(
                "SELECT id, driver, definition_name, subnet_cidr, runtime_dir, helper_pid,
                        transport_socket_path, log_path, pid_file_path, pcap_path, state, created_at, modified_at
                 FROM network_instances WHERE id = ?1",
                params![network_id],
                row_to_network_instance,
            )
            .optional()
            .map_err(LibVmError::from)
    }

    pub fn upsert_network_instance(
        &self,
        instance: &NetworkInstanceState,
    ) -> Result<(), LibVmError> {
        self.conn.execute(
            "INSERT INTO network_instances
                (id, driver, definition_name, subnet_cidr, runtime_dir, helper_pid,
                 transport_socket_path, log_path, pid_file_path, pcap_path, state, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                driver = excluded.driver,
                definition_name = excluded.definition_name,
                subnet_cidr = excluded.subnet_cidr,
                runtime_dir = excluded.runtime_dir,
                helper_pid = excluded.helper_pid,
                transport_socket_path = excluded.transport_socket_path,
                log_path = excluded.log_path,
                pid_file_path = excluded.pid_file_path,
                pcap_path = excluded.pcap_path,
                state = excluded.state,
                modified_at = excluded.modified_at",
            params![
                instance.id,
                instance.driver,
                instance.definition_name,
                instance.subnet_cidr,
                instance.runtime_dir,
                instance.helper_pid,
                instance.transport_socket_path,
                instance.log_path,
                instance.pid_file_path,
                instance.pcap_path,
                instance.state,
                instance.created_at,
                instance.modified_at,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_network_attachment(
        &self,
        attachment: &NetworkAttachmentState,
    ) -> Result<(), LibVmError> {
        self.conn.execute(
            "INSERT INTO network_attachments
                (machine_id, network_instance_id, guest_mac, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(machine_id) DO UPDATE SET
                network_instance_id = excluded.network_instance_id,
                guest_mac = excluded.guest_mac,
                modified_at = excluded.modified_at",
            params![
                attachment.machine_id.to_string(),
                attachment.network_instance_id,
                attachment.guest_mac,
                attachment.created_at,
                attachment.modified_at,
            ],
        )?;
        Ok(())
    }

    pub fn remove_network_attachment(&self, machine_id: MachineId) -> Result<(), LibVmError> {
        self.conn.execute(
            "DELETE FROM network_attachments WHERE machine_id = ?1",
            params![machine_id.to_string()],
        )?;
        Ok(())
    }

    pub fn remove_network_instance(&self, network_id: &str) -> Result<(), LibVmError> {
        self.conn.execute(
            "DELETE FROM network_instances WHERE id = ?1",
            params![network_id],
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
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS machines (
            id           TEXT PRIMARY KEY,
            name         TEXT NOT NULL UNIQUE,
            instance_dir TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            modified_at  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS network_instances (
            id                      TEXT PRIMARY KEY,
            driver                  TEXT NOT NULL,
            definition_name         TEXT,
            subnet_cidr             TEXT NOT NULL,
            runtime_dir             TEXT NOT NULL,
            helper_pid              INTEGER NOT NULL,
            transport_socket_path   TEXT NOT NULL,
            log_path                TEXT NOT NULL,
            pid_file_path           TEXT NOT NULL,
            pcap_path               TEXT,
            state                   TEXT NOT NULL,
            created_at              INTEGER NOT NULL,
            modified_at             INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS network_attachments (
            machine_id              TEXT NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
            network_instance_id     TEXT NOT NULL REFERENCES network_instances(id) ON DELETE CASCADE,
            guest_mac               TEXT NOT NULL,
            created_at              INTEGER NOT NULL,
            modified_at             INTEGER NOT NULL,
            PRIMARY KEY (machine_id)
        );
        CREATE TRIGGER IF NOT EXISTS machines_created_at_immutable
        BEFORE UPDATE OF created_at ON machines
        BEGIN
            SELECT RAISE(ABORT, 'machines.created_at is immutable');
        END;
        CREATE TRIGGER IF NOT EXISTS network_instances_created_at_immutable
        BEFORE UPDATE OF created_at ON network_instances
        BEGIN
            SELECT RAISE(ABORT, 'network_instances.created_at is immutable');
        END;
        CREATE TRIGGER IF NOT EXISTS network_attachments_created_at_immutable
        BEFORE UPDATE OF created_at ON network_attachments
        BEGIN
            SELECT RAISE(ABORT, 'network_attachments.created_at is immutable');
        END;
        PRAGMA user_version = 1;",
    )?;

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
        created_at: row.get(3)?,
        modified_at: row.get(4)?,
    })
}

fn row_to_network_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<NetworkInstanceState> {
    Ok(NetworkInstanceState {
        id: row.get(0)?,
        driver: row.get(1)?,
        definition_name: row.get(2)?,
        subnet_cidr: row.get(3)?,
        runtime_dir: row.get(4)?,
        helper_pid: row.get(5)?,
        transport_socket_path: row.get(6)?,
        log_path: row.get(7)?,
        pid_file_path: row.get(8)?,
        pcap_path: row.get(9)?,
        state: row.get(10)?,
        created_at: row.get(11)?,
        modified_at: row.get(12)?,
    })
}

fn row_to_network_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<NetworkAttachmentState> {
    let id_str: String = row.get(0)?;
    let machine_id: MachineId = id_str.parse().map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(NetworkAttachmentState {
        machine_id,
        network_instance_id: row.get(1)?,
        guest_mac: row.get(2)?,
        created_at: row.get(3)?,
        modified_at: row.get(4)?,
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs() as i64
}

pub fn metadata_from_path(id: MachineId, name: String, instance_dir: &Path) -> MachineMetadata {
    let now = now_unix();
    MachineMetadata {
        id,
        name,
        instance_dir: instance_dir.display().to_string(),
        created_at: now,
        modified_at: now,
    }
}

#[cfg(test)]
mod tests {
    use bento_core::MachineId;

    use super::{metadata_from_path, NetworkAttachmentState, NetworkInstanceState, StateStore};
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
    fn network_instance_and_attachment_round_trip_and_remove() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");
        let id = MachineId::new();
        let metadata = metadata_from_path(id, "netbox".to_string(), &layout.instance_dir(id));
        store.insert_machine(&metadata).expect("insert machine");

        let network_id = "netbox-runtime".to_string();
        let instance = NetworkInstanceState {
            id: network_id.clone(),
            driver: "gvisor".to_string(),
            definition_name: None,
            subnet_cidr: "192.168.105.0/24".to_string(),
            runtime_dir: "/tmp/netbox-runtime".to_string(),
            helper_pid: 1234,
            transport_socket_path: "/tmp/gvproxy.sock".to_string(),
            log_path: "/tmp/gvproxy.log".to_string(),
            pid_file_path: "/tmp/gvproxy.pid".to_string(),
            pcap_path: None,
            state: "running".to_string(),
            created_at: 41,
            modified_at: 42,
        };
        let attachment = NetworkAttachmentState {
            machine_id: id,
            network_instance_id: network_id.clone(),
            guest_mac: "02:11:22:33:44:55".to_string(),
            created_at: 43,
            modified_at: 44,
        };

        store
            .upsert_network_instance(&instance)
            .expect("upsert network instance");
        store
            .upsert_network_attachment(&attachment)
            .expect("upsert network attachment");
        assert_eq!(
            store
                .get_network_instance(&network_id)
                .expect("get network instance")
                .expect("network instance exists"),
            instance
        );
        assert_eq!(
            store
                .get_network_attachment(id)
                .expect("get network attachment")
                .expect("network attachment exists"),
            attachment
        );

        store
            .remove_network_attachment(id)
            .expect("remove network attachment");
        assert!(store
            .get_network_attachment(id)
            .expect("get network attachment")
            .is_none());
        store
            .remove_network_instance(&network_id)
            .expect("remove network instance");
        assert!(store
            .get_network_instance(&network_id)
            .expect("get network instance")
            .is_none());
    }

    #[test]
    fn created_at_columns_are_immutable() {
        let (_dir, layout) = temp_layout();
        let store = StateStore::open(&layout).expect("open store");
        let id = MachineId::new();
        let metadata = metadata_from_path(id, "immutable".to_string(), &layout.instance_dir(id));
        store.insert_machine(&metadata).expect("insert machine");

        let network_id = "immutable-runtime".to_string();
        store
            .upsert_network_instance(&NetworkInstanceState {
                id: network_id.clone(),
                driver: "gvisor".to_string(),
                definition_name: None,
                subnet_cidr: "192.168.105.0/24".to_string(),
                runtime_dir: "/tmp/immutable-runtime".to_string(),
                helper_pid: 1234,
                transport_socket_path: "/tmp/gvproxy.sock".to_string(),
                log_path: "/tmp/gvproxy.log".to_string(),
                pid_file_path: "/tmp/gvproxy.pid".to_string(),
                pcap_path: None,
                state: "running".to_string(),
                created_at: 41,
                modified_at: 42,
            })
            .expect("insert network instance");
        store
            .upsert_network_attachment(&NetworkAttachmentState {
                machine_id: id,
                network_instance_id: network_id.clone(),
                guest_mac: "02:11:22:33:44:55".to_string(),
                created_at: 43,
                modified_at: 44,
            })
            .expect("insert network attachment");

        let result = store.conn.execute(
            "UPDATE machines SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![metadata.created_at + 1, id.to_string()],
        );
        assert!(result.is_err(), "created_at update should be rejected");
        let result = store.conn.execute(
            "UPDATE network_instances SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![42, network_id],
        );
        assert!(result.is_err(), "created_at update should be rejected");
        let result = store.conn.execute(
            "UPDATE network_attachments SET created_at = ?1 WHERE machine_id = ?2",
            rusqlite::params![44, id.to_string()],
        );
        assert!(result.is_err(), "created_at update should be rejected");
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
