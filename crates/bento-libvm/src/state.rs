use std::fs;
use std::path::Path;

use bento_core::MachineId;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::{Layout, LibVmError};

const NAME_BY_ID: TableDefinition<&str, &str> = TableDefinition::new("name_by_id");
const ID_BY_NAME: TableDefinition<&str, &str> = TableDefinition::new("id_by_name");
const INSTANCE_DIR_BY_ID: TableDefinition<&str, &str> = TableDefinition::new("instance_dir_by_id");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineMetadata {
    pub id: MachineId,
    pub name: String,
    pub instance_dir: String,
}

pub struct StateStore {
    db: Database,
}

impl StateStore {
    pub fn open(layout: &Layout) -> Result<Self, LibVmError> {
        fs::create_dir_all(layout.data_dir())?;
        let db = Database::create(layout.state_db_path())?;
        let store = Self { db };
        store.initialize_tables()?;
        Ok(store)
    }

    fn initialize_tables(&self) -> Result<(), LibVmError> {
        let write_txn = self.db.begin_write()?;
        write_txn.open_table(NAME_BY_ID)?;
        write_txn.open_table(ID_BY_NAME)?;
        write_txn.open_table(INSTANCE_DIR_BY_ID)?;
        write_txn.commit()?;
        Ok(())
    }

    pub fn insert_machine(&self, metadata: &MachineMetadata) -> Result<(), LibVmError> {
        let write_txn = self.db.begin_write()?;

        {
            let name_by_id = write_txn.open_table(NAME_BY_ID)?;
            if name_by_id.get(metadata.id.to_string().as_str())?.is_some() {
                return Err(LibVmError::MachineIdAlreadyExists { id: metadata.id });
            }
        }

        {
            let id_by_name = write_txn.open_table(ID_BY_NAME)?;
            if id_by_name.get(metadata.name.as_str())?.is_some() {
                return Err(LibVmError::MachineAlreadyExists {
                    name: metadata.name.clone(),
                });
            }
        }

        {
            let mut name_by_id = write_txn.open_table(NAME_BY_ID)?;
            let id = metadata.id.to_string();
            name_by_id.insert(id.as_str(), metadata.name.as_str())?;
        }
        {
            let mut id_by_name = write_txn.open_table(ID_BY_NAME)?;
            let id = metadata.id.to_string();
            id_by_name.insert(metadata.name.as_str(), id.as_str())?;
        }
        {
            let mut instance_dir_by_id = write_txn.open_table(INSTANCE_DIR_BY_ID)?;
            let id = metadata.id.to_string();
            instance_dir_by_id.insert(id.as_str(), metadata.instance_dir.as_str())?;
        }

        write_txn.commit()?;
        Ok(())
    }

    pub fn get_machine_by_id(&self, id: MachineId) -> Result<Option<MachineMetadata>, LibVmError> {
        let read_txn = self.db.begin_read()?;
        let name_by_id = read_txn.open_table(NAME_BY_ID)?;
        let instance_dir_by_id = read_txn.open_table(INSTANCE_DIR_BY_ID)?;

        let id_string = id.to_string();
        let Some(name) = name_by_id.get(id_string.as_str())? else {
            return Ok(None);
        };
        let Some(instance_dir) = instance_dir_by_id.get(id_string.as_str())? else {
            return Err(LibVmError::CorruptState {
                id,
                field: "instance_dir",
            });
        };

        Ok(Some(MachineMetadata {
            id,
            name: name.value().to_string(),
            instance_dir: instance_dir.value().to_string(),
        }))
    }

    pub fn get_machine_by_name(&self, name: &str) -> Result<Option<MachineMetadata>, LibVmError> {
        let read_txn = self.db.begin_read()?;
        let id_by_name = read_txn.open_table(ID_BY_NAME)?;

        let Some(id) = id_by_name.get(name)? else {
            return Ok(None);
        };

        let parsed: MachineId = id.value().parse().map_err(|_| LibVmError::CorruptState {
            id: MachineId::new(),
            field: "id",
        })?;
        drop(id_by_name);

        self.get_machine_by_id(parsed)
    }

    pub fn list_machines(&self) -> Result<Vec<MachineMetadata>, LibVmError> {
        let read_txn = self.db.begin_read()?;
        let name_by_id = read_txn.open_table(NAME_BY_ID)?;
        let instance_dir_by_id = read_txn.open_table(INSTANCE_DIR_BY_ID)?;
        let mut machines = Vec::new();

        for row in name_by_id.iter()? {
            let (id, name) = row?;
            let id_string = id.value();
            let parsed: MachineId = id_string.parse().map_err(|_| LibVmError::CorruptState {
                id: MachineId::new(),
                field: "id",
            })?;

            let Some(instance_dir) = instance_dir_by_id.get(id_string)? else {
                return Err(LibVmError::CorruptState {
                    id: parsed,
                    field: "instance_dir",
                });
            };

            machines.push(MachineMetadata {
                id: parsed,
                name: name.value().to_string(),
                instance_dir: instance_dir.value().to_string(),
            });
        }

        machines.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(machines)
    }

    pub fn remove_machine(&self, metadata: &MachineMetadata) -> Result<(), LibVmError> {
        let write_txn = self.db.begin_write()?;
        let id_string = metadata.id.to_string();

        write_txn
            .open_table(NAME_BY_ID)?
            .remove(id_string.as_str())?;
        write_txn
            .open_table(ID_BY_NAME)?
            .remove(metadata.name.as_str())?;
        write_txn
            .open_table(INSTANCE_DIR_BY_ID)?
            .remove(id_string.as_str())?;

        write_txn.commit()?;
        Ok(())
    }
}

pub fn metadata_from_path(id: MachineId, name: String, instance_dir: &Path) -> MachineMetadata {
    MachineMetadata {
        id,
        name,
        instance_dir: instance_dir.display().to_string(),
    }
}
