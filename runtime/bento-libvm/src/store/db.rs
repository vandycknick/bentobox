use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bento_core::{MachineId, VmSpec};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::models::{
    Machine, MachineRuntime, NetworkAttachment, NetworkDefinition, NetworkInstance,
    RequestedNetwork,
};
use crate::store::wrappers::{
    DbMachine, DbMachineRuntime, DbNetworkAttachment, DbNetworkDefinition, DbNetworkInstance,
};
use crate::store::Database;
use crate::{Layout, LibVmError};

const MACHINE_COLUMNS: &str =
    "id, name, json(config_json) AS config_json, instance_dir, created_at, modified_at, image_ref, json(labels) AS labels, json(metadata) AS metadata, json(network) AS network";

#[derive(Debug, Clone)]
pub(crate) struct Sqlite {
    pool: SqlitePool,
}

impl Sqlite {
    async fn setup_db(pool: &SqlitePool) -> Result<(), LibVmError> {
        sqlx::migrate!("./migrations").run(pool).await?;
        Ok(())
    }
}

impl Database for Sqlite {
    type Settings = Layout;

    async fn new(layout: &Self::Settings) -> Result<Self, LibVmError> {
        std::fs::create_dir_all(layout.data_dir())?;
        let options = sqlite_options(&layout.state_db_path());
        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(options)
            .await?;
        Self::setup_db(&pool).await?;
        Ok(Self { pool })
    }

    async fn insert_machine(&self, machine: &Machine) -> Result<(), LibVmError> {
        sqlx::query(
            "INSERT INTO machines (id, name, config_json, instance_dir, created_at, modified_at, image_ref, labels, metadata, network)
             VALUES (?1, ?2, jsonb(?3), ?4, ?5, ?6, ?7, jsonb(?8), jsonb(?9), jsonb(?10))",
        )
        .bind(machine.id.to_string())
        .bind(&machine.name)
        .bind(serialize_vm_spec("config_json", &machine.config)?)
        .bind(&machine.instance_dir)
        .bind(machine.created_at)
        .bind(machine.modified_at)
        .bind(&machine.image_ref)
        .bind(serialize_map("labels", &machine.labels)?)
        .bind(serialize_map("metadata", &machine.metadata)?)
        .bind(serialize_network(&machine.network)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_machine_runtime(
        &self,
        machine_id: MachineId,
    ) -> Result<Option<MachineRuntime>, LibVmError> {
        let runtime = sqlx::query_as::<_, DbMachineRuntime>(
            "SELECT machine_id, state, vmmon_pid, started_at, last_error, updated_at
             FROM machine_runtime WHERE machine_id = ?1",
        )
        .bind(machine_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(runtime.map(|DbMachineRuntime(runtime)| runtime))
    }

    async fn upsert_machine_runtime(&self, runtime: &MachineRuntime) -> Result<(), LibVmError> {
        sqlx::query(
            "INSERT INTO machine_runtime
                (machine_id, state, vmmon_pid, started_at, last_error, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(machine_id) DO UPDATE SET
                state = excluded.state,
                vmmon_pid = excluded.vmmon_pid,
                started_at = excluded.started_at,
                last_error = excluded.last_error,
                updated_at = excluded.updated_at",
        )
        .bind(runtime.machine_id.to_string())
        .bind(runtime.state.as_str())
        .bind(runtime.vmmon_pid)
        .bind(runtime.started_at)
        .bind(&runtime.last_error)
        .bind(runtime.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_machine_runtime(&self, machine_id: MachineId) -> Result<(), LibVmError> {
        sqlx::query("DELETE FROM machine_runtime WHERE machine_id = ?1")
            .bind(machine_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_machine_network(
        &self,
        machine_id: MachineId,
        network: &RequestedNetwork,
    ) -> Result<(), LibVmError> {
        sqlx::query("UPDATE machines SET network = jsonb(?1), modified_at = ?2 WHERE id = ?3")
            .bind(serialize_network(network)?)
            .bind(now_unix())
            .bind(machine_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_machine_config(
        &self,
        machine_id: MachineId,
        config: &VmSpec,
    ) -> Result<(), LibVmError> {
        sqlx::query("UPDATE machines SET config_json = jsonb(?1), modified_at = ?2 WHERE id = ?3")
            .bind(serialize_vm_spec("config_json", config)?)
            .bind(now_unix())
            .bind(machine_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_machine_by_id(&self, id: MachineId) -> Result<Option<Machine>, LibVmError> {
        let query = format!("SELECT {MACHINE_COLUMNS} FROM machines WHERE id = ?1");
        let machine = sqlx::query_as::<_, DbMachine>(&query)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(machine.map(|DbMachine(machine)| machine))
    }

    async fn get_machine_by_name(&self, name: &str) -> Result<Option<Machine>, LibVmError> {
        let query = format!("SELECT {MACHINE_COLUMNS} FROM machines WHERE name = ?1");
        let machine = sqlx::query_as::<_, DbMachine>(&query)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(machine.map(|DbMachine(machine)| machine))
    }

    async fn get_machine_by_id_prefix(&self, prefix: &str) -> Result<Vec<Machine>, LibVmError> {
        let pattern = format!("{prefix}%");
        let query = format!("SELECT {MACHINE_COLUMNS} FROM machines WHERE id LIKE ?1");
        let rows = sqlx::query_as::<_, DbMachine>(&query)
            .bind(pattern)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|DbMachine(machine)| machine).collect())
    }

    async fn list_machines(&self) -> Result<Vec<Machine>, LibVmError> {
        let query = format!("SELECT {MACHINE_COLUMNS} FROM machines ORDER BY name");
        let rows = sqlx::query_as::<_, DbMachine>(&query)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|DbMachine(machine)| machine).collect())
    }

    async fn allocate_ephemeral_name(&self, prefix: &str) -> Result<String, LibVmError> {
        for index in 1..10_000u32 {
            let candidate = format!("{prefix}-{index}");
            if self.get_machine_by_name(&candidate).await?.is_none() {
                return Ok(candidate);
            }
        }

        Err(LibVmError::InvalidMachineName {
            name: prefix.to_string(),
            reason: "failed to allocate ephemeral VM name".to_string(),
        })
    }

    async fn remove_machine(&self, machine: &Machine) -> Result<(), LibVmError> {
        sqlx::query("DELETE FROM machines WHERE id = ?1")
            .bind(machine.id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_network_attachment(
        &self,
        machine_id: MachineId,
    ) -> Result<Option<NetworkAttachment>, LibVmError> {
        let attachment = sqlx::query_as::<_, DbNetworkAttachment>(
            "SELECT machine_id, network_instance_id, guest_mac, created_at, modified_at
             FROM network_attachments WHERE machine_id = ?1",
        )
        .bind(machine_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(attachment.map(|DbNetworkAttachment(attachment)| attachment))
    }

    async fn get_network_instance(
        &self,
        network_id: &str,
    ) -> Result<Option<NetworkInstance>, LibVmError> {
        let instance = sqlx::query_as::<_, DbNetworkInstance>(
            "SELECT id, driver, definition_name, runtime_dir, json(attachment_json) AS attachment_json,
                    json(driver_state_json) AS driver_state_json, state, created_at, modified_at
             FROM network_instances WHERE id = ?1",
        )
        .bind(network_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(instance.map(|DbNetworkInstance(instance)| instance))
    }

    async fn upsert_network_instance(&self, instance: &NetworkInstance) -> Result<(), LibVmError> {
        sqlx::query(
            "INSERT INTO network_instances
                (id, driver, definition_name, runtime_dir, attachment_json, driver_state_json,
                 state, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, jsonb(?5), jsonb(?6), ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                driver = excluded.driver,
                definition_name = excluded.definition_name,
                runtime_dir = excluded.runtime_dir,
                attachment_json = excluded.attachment_json,
                driver_state_json = excluded.driver_state_json,
                state = excluded.state,
                modified_at = excluded.modified_at",
        )
        .bind(&instance.id)
        .bind(&instance.driver)
        .bind(&instance.definition_name)
        .bind(&instance.runtime_dir)
        .bind(&instance.attachment_json)
        .bind(&instance.driver_state_json)
        .bind(&instance.state)
        .bind(instance.created_at)
        .bind(instance.modified_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn upsert_network_attachment(
        &self,
        attachment: &NetworkAttachment,
    ) -> Result<(), LibVmError> {
        sqlx::query(
            "INSERT INTO network_attachments
                (machine_id, network_instance_id, guest_mac, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(machine_id) DO UPDATE SET
                network_instance_id = excluded.network_instance_id,
                guest_mac = excluded.guest_mac,
                modified_at = excluded.modified_at",
        )
        .bind(attachment.machine_id.to_string())
        .bind(&attachment.network_instance_id)
        .bind(&attachment.guest_mac)
        .bind(attachment.created_at)
        .bind(attachment.modified_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_network_attachment(&self, machine_id: MachineId) -> Result<(), LibVmError> {
        sqlx::query("DELETE FROM network_attachments WHERE machine_id = ?1")
            .bind(machine_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn remove_network_instance(&self, network_id: &str) -> Result<(), LibVmError> {
        sqlx::query("DELETE FROM network_instances WHERE id = ?1")
            .bind(network_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_network_instance_by_definition(
        &self,
        definition_name: &str,
    ) -> Result<Option<NetworkInstance>, LibVmError> {
        let instance = sqlx::query_as::<_, DbNetworkInstance>(
            "SELECT id, driver, definition_name, runtime_dir, json(attachment_json) AS attachment_json,
                    json(driver_state_json) AS driver_state_json, state, created_at, modified_at
             FROM network_instances WHERE definition_name = ?1",
        )
        .bind(definition_name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(instance.map(|DbNetworkInstance(instance)| instance))
    }

    async fn count_network_attachments(&self, network_id: &str) -> Result<u32, LibVmError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM network_attachments WHERE network_instance_id = ?1",
        )
        .bind(network_id)
        .fetch_one(&self.pool)
        .await?;
        u32::try_from(count).map_err(|err| LibVmError::StateDecode {
            field: "network_attachments.count",
            message: err.to_string(),
        })
    }

    async fn upsert_network_definition(
        &self,
        definition: &NetworkDefinition,
    ) -> Result<(), LibVmError> {
        let now = now_unix();
        sqlx::query(
            "INSERT INTO network_definitions (name, mode, driver_preference, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET
                mode = excluded.mode,
                driver_preference = excluded.driver_preference,
                modified_at = excluded.modified_at",
        )
        .bind(&definition.name)
        .bind(serde_json::to_string(&definition.mode).map_err(|err| {
            LibVmError::InvalidCreateRequest {
                name: definition.name.clone(),
                reason: format!("serialize network mode: {err}"),
            }
        })?)
        .bind(serde_json::to_string(&definition.driver_preference).map_err(|err| {
            LibVmError::InvalidCreateRequest {
                name: definition.name.clone(),
                reason: format!("serialize network driver preference: {err}"),
            }
        })?)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_network_definitions(&self) -> Result<Vec<NetworkDefinition>, LibVmError> {
        let rows = sqlx::query_as::<_, DbNetworkDefinition>(
            "SELECT name, mode, driver_preference, created_at, modified_at
             FROM network_definitions ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|DbNetworkDefinition(definition)| definition)
            .collect())
    }

    async fn get_network_definition(
        &self,
        name: &str,
    ) -> Result<Option<NetworkDefinition>, LibVmError> {
        let definition = sqlx::query_as::<_, DbNetworkDefinition>(
            "SELECT name, mode, driver_preference, created_at, modified_at
             FROM network_definitions WHERE name = ?1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(definition.map(|DbNetworkDefinition(definition)| definition))
    }

    async fn remove_network_definition(&self, name: &str) -> Result<(), LibVmError> {
        sqlx::query("DELETE FROM network_definitions WHERE name = ?1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn sqlite_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
}

fn serialize_map(
    field: &'static str,
    values: &BTreeMap<String, String>,
) -> Result<String, LibVmError> {
    serde_json::to_string(values).map_err(|err| LibVmError::InvalidCreateRequest {
        name: field.to_string(),
        reason: format!("serialize {field}: {err}"),
    })
}

fn serialize_network(network: &RequestedNetwork) -> Result<String, LibVmError> {
    serde_json::to_string(network).map_err(|err| LibVmError::InvalidCreateRequest {
        name: "network".to_string(),
        reason: format!("serialize network: {err}"),
    })
}

fn serialize_vm_spec(field: &'static str, spec: &VmSpec) -> Result<String, LibVmError> {
    serde_json::to_string(spec).map_err(|err| LibVmError::InvalidCreateRequest {
        name: field.to_string(),
        reason: format!("serialize {field}: {err}"),
    })
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use bento_core::{
        Architecture, Boot, GuestOs, MachineId, Platform, Resources, Settings, Storage, VmSpec,
    };

    use crate::models::{
        Machine, MachineRuntime, MachineRuntimeState, NetworkAttachment, NetworkInstance,
        RequestedNetwork,
    };
    use crate::store::{Database, Sqlite};
    use crate::Layout;

    fn temp_layout() -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let layout = Layout::new(dir.path());
        (dir, layout)
    }

    fn machine_from_path(id: MachineId, name: String, instance_dir: &Path) -> Machine {
        let config = sample_vm_spec();
        Machine {
            id,
            name,
            config,
            instance_dir: instance_dir.display().to_string(),
            created_at: 1,
            modified_at: 1,
            image_ref: String::new(),
            labels: BTreeMap::new(),
            metadata: BTreeMap::new(),
            network: RequestedNetwork::default(),
        }
    }

    fn sample_vm_spec() -> VmSpec {
        VmSpec {
            version: 1,
            platform: Platform {
                guest_os: GuestOs::Linux,
                architecture: Architecture::Aarch64,
            },
            resources: Resources {
                cpus: 2,
                memory_mib: 1024,
            },
            boot: Boot {
                kernel: None,
                initramfs: None,
                kernel_cmdline: Vec::new(),
                bootstrap: None,
            },
            storage: Storage { disks: Vec::new() },
            mounts: Vec::new(),
            vsock_endpoints: Vec::new(),
            settings: Settings {
                nested_virtualization: false,
                rosetta: false,
            },
            guest: None,
        }
    }

    #[tokio::test]
    async fn insert_and_lookup_by_name() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "devbox".to_string(), &layout.instance_dir(id));

        db.insert_machine(&metadata).await.expect("insert");
        let found = db
            .get_machine_by_name("devbox")
            .await
            .expect("lookup")
            .expect("should find machine");

        assert_eq!(found, metadata);
    }

    #[tokio::test]
    async fn insert_and_lookup_by_id() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "testvm".to_string(), &layout.instance_dir(id));

        db.insert_machine(&metadata).await.expect("insert");
        let found = db
            .get_machine_by_id(id)
            .await
            .expect("lookup")
            .expect("should find machine");

        assert_eq!(found, metadata);
    }

    #[tokio::test]
    async fn lookup_by_id_prefix() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "prefix-test".to_string(), &layout.instance_dir(id));

        db.insert_machine(&metadata).await.expect("insert");

        let id_str = id.to_string();
        let prefix = &id_str[..8];
        let found = db.get_machine_by_id_prefix(prefix).await.expect("lookup");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0], metadata);
    }

    #[tokio::test]
    async fn labels_metadata_and_network_round_trip_as_jsonb_blobs() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let mut labels = BTreeMap::new();
        labels.insert("owner".to_string(), "test".to_string());
        let mut metadata = BTreeMap::new();
        metadata.insert("bento.profile".to_string(), "rust-dev".to_string());

        let machine = Machine {
            id,
            name: "jsonb-test".to_string(),
            config: sample_vm_spec(),
            instance_dir: layout.instance_dir(id).display().to_string(),
            created_at: 1,
            modified_at: 1,
            image_ref: "test-image:latest".to_string(),
            labels,
            metadata,
            network: RequestedNetwork::default(),
        };

        db.insert_machine(&machine).await.expect("insert machine");
        let found = db
            .get_machine_by_id(id)
            .await
            .expect("lookup")
            .expect("machine exists");

        assert_eq!(found.labels.get("owner").map(String::as_str), Some("test"));
        assert_eq!(
            found.metadata.get("bento.profile").map(String::as_str),
            Some("rust-dev")
        );
        assert_eq!(found.network, RequestedNetwork::default());
        let storage_type: String =
            sqlx::query_scalar("SELECT typeof(labels) FROM machines WHERE id = ?1")
                .bind(id.to_string())
                .fetch_one(&db.pool)
                .await
                .expect("query storage type");
        assert_eq!(storage_type, "blob");
    }

    #[tokio::test]
    async fn list_machines_sorted_by_name() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");

        let id_b = MachineId::new();
        let id_a = MachineId::new();
        db.insert_machine(&machine_from_path(
            id_b,
            "bravo".to_string(),
            &layout.instance_dir(id_b),
        ))
        .await
        .expect("insert b");
        db.insert_machine(&machine_from_path(
            id_a,
            "alpha".to_string(),
            &layout.instance_dir(id_a),
        ))
        .await
        .expect("insert a");

        let list = db.list_machines().await.expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "bravo");
    }

    #[tokio::test]
    async fn remove_machine() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "gonner".to_string(), &layout.instance_dir(id));

        db.insert_machine(&metadata).await.expect("insert");
        db.remove_machine(&metadata).await.expect("remove");

        let found = db.get_machine_by_id(id).await.expect("lookup");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn machine_runtime_round_trips() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "runtime".to_string(), &layout.instance_dir(id));
        db.insert_machine(&metadata).await.expect("insert");

        let runtime = MachineRuntime {
            machine_id: id,
            state: MachineRuntimeState::Running,
            vmmon_pid: Some(1234),
            started_at: Some(42),
            last_error: None,
            updated_at: 43,
        };
        db.upsert_machine_runtime(&runtime)
            .await
            .expect("upsert runtime");

        assert_eq!(
            db.get_machine_runtime(id)
                .await
                .expect("get runtime")
                .expect("runtime exists"),
            runtime
        );

        db.remove_machine_runtime(id).await.expect("remove runtime");
        assert!(db
            .get_machine_runtime(id)
            .await
            .expect("get runtime")
            .is_none());
    }

    #[tokio::test]
    async fn update_machine_config_persists_config_json() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "config".to_string(), &layout.instance_dir(id));
        db.insert_machine(&metadata).await.expect("insert");

        let mut updated = metadata.config.clone();
        updated.resources.cpus = 8;
        db.update_machine_config(id, &updated)
            .await
            .expect("update config");

        let found = db
            .get_machine_by_id(id)
            .await
            .expect("lookup")
            .expect("machine exists");
        assert_eq!(found.config.resources.cpus, 8);
    }

    #[tokio::test]
    async fn network_instance_and_attachment_round_trip_and_remove() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "netbox".to_string(), &layout.instance_dir(id));
        db.insert_machine(&metadata).await.expect("insert machine");

        let network_id = "netbox-runtime".to_string();
        let instance = NetworkInstance {
            id: network_id.clone(),
            driver: "netd".to_string(),
            definition_name: None,
            runtime_dir: "/tmp/netbox-runtime".to_string(),
            attachment_json: r#"{"kind":"none"}"#.to_string(),
            driver_state_json: r#"{"helper_pid":1234}"#.to_string(),
            state: "running".to_string(),
            created_at: 41,
            modified_at: 42,
        };
        let attachment = NetworkAttachment {
            machine_id: id,
            network_instance_id: network_id.clone(),
            guest_mac: "02:11:22:33:44:55".to_string(),
            created_at: 43,
            modified_at: 44,
        };

        db.upsert_network_instance(&instance)
            .await
            .expect("upsert network instance");
        db.upsert_network_attachment(&attachment)
            .await
            .expect("upsert network attachment");
        assert_eq!(
            db.get_network_instance(&network_id)
                .await
                .expect("get network instance")
                .expect("network instance exists"),
            instance
        );
        assert_eq!(
            db.get_network_attachment(id)
                .await
                .expect("get network attachment")
                .expect("network attachment exists"),
            attachment
        );

        db.remove_network_attachment(id)
            .await
            .expect("remove network attachment");
        assert!(db
            .get_network_attachment(id)
            .await
            .expect("get network attachment")
            .is_none());
        db.remove_network_instance(&network_id)
            .await
            .expect("remove network instance");
        assert!(db
            .get_network_instance(&network_id)
            .await
            .expect("get network instance")
            .is_none());
    }

    #[tokio::test]
    async fn created_at_columns_are_immutable() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");
        let id = MachineId::new();
        let metadata = machine_from_path(id, "immutable".to_string(), &layout.instance_dir(id));
        db.insert_machine(&metadata).await.expect("insert machine");

        let result = sqlx::query("UPDATE machines SET created_at = ?1 WHERE id = ?2")
            .bind(metadata.created_at + 1)
            .bind(id.to_string())
            .execute(&db.pool)
            .await;
        assert!(result.is_err(), "created_at update should be rejected");
    }

    #[tokio::test]
    async fn duplicate_name_fails() {
        let (_dir, layout) = temp_layout();
        let db = Sqlite::new(&layout).await.expect("open db");

        let id1 = MachineId::new();
        let id2 = MachineId::new();
        db.insert_machine(&machine_from_path(
            id1,
            "dupe".to_string(),
            &layout.instance_dir(id1),
        ))
        .await
        .expect("insert first");

        let result = db
            .insert_machine(&machine_from_path(
                id2,
                "dupe".to_string(),
                &layout.instance_dir(id2),
            ))
            .await;
        assert!(result.is_err(), "duplicate name should fail");
    }

    #[tokio::test]
    async fn concurrent_connections_work() {
        let (_dir, layout) = temp_layout();
        let db1 = Sqlite::new(&layout).await.expect("open db 1");
        let db2 = Sqlite::new(&layout).await.expect("open db 2");

        let id = MachineId::new();
        db1.insert_machine(&machine_from_path(
            id,
            "shared".to_string(),
            &layout.instance_dir(id),
        ))
        .await
        .expect("insert via db1");

        let found = db2
            .get_machine_by_name("shared")
            .await
            .expect("lookup via db2")
            .expect("should find machine from other connection");

        assert_eq!(found.id, id);
    }
}
