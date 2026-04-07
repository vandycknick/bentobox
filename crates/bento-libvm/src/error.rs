use std::path::PathBuf;

use bento_core::MachineId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LibVmError {
    #[error("could not resolve Bento data directory from XDG_DATA_HOME or HOME")]
    DataDirUnavailable,

    #[error("environment variable {name} must be an absolute path, got {path}")]
    RelativeEnvironmentPath { name: &'static str, path: PathBuf },

    #[error("invalid machine name {name:?}: {reason}")]
    InvalidMachineName { name: String, reason: String },

    #[error("machine {name:?} already exists")]
    MachineAlreadyExists { name: String },

    #[error("machine {reference} not found")]
    MachineNotFound { reference: String },

    #[error("machine {id} already exists")]
    MachineIdAlreadyExists { id: MachineId },

    #[error("machine {id} metadata is missing required field {field}")]
    CorruptState { id: MachineId, field: &'static str },

    #[error("failed to serialize VmSpec for machine {name:?}")]
    VmSpecSerializeFailed {
        name: String,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("failed to load VmSpec for machine {id} from {path}")]
    VmSpecLoadFailed {
        id: MachineId,
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("redb state error: {0}")]
    StateStore(#[from] redb::Error),

    #[error(transparent)]
    StateDatabase(#[from] redb::DatabaseError),

    #[error(transparent)]
    StateStorage(#[from] redb::StorageError),

    #[error(transparent)]
    StateTransaction(#[from] redb::TransactionError),

    #[error(transparent)]
    StateTable(#[from] redb::TableError),

    #[error(transparent)]
    StateCommit(#[from] redb::CommitError),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
