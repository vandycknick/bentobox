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

    #[error("machine {reference} is already running")]
    MachineAlreadyRunning { reference: String },

    #[error("machine {reference} is not running")]
    MachineNotRunning { reference: String },

    #[error("monitor connection for {reference} failed: {message}")]
    MonitorConnection { reference: String, message: String },

    #[error("monitor protocol for {reference} failed: {message}")]
    MonitorProtocol { reference: String, message: String },

    #[error(
        "vmmon executable not found. Expected a sibling binary at {expected_path} or `vmmon` in PATH. Build it with `cargo build -p bento-vmmon` (or `cargo build --release -p bento-vmmon`)."
    )]
    VmMonExecutableNotFound { expected_path: PathBuf },

    #[error("invalid create request for machine {name:?}: {reason}")]
    InvalidCreateRequest { name: String, reason: String },

    #[error("unsupported image architecture {arch:?}")]
    UnsupportedImageArchitecture { arch: String },

    #[error("unsupported image guest OS {os:?}")]
    UnsupportedImageGuestOs { os: String },

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

    #[error("ambiguous machine id prefix {prefix:?} matched {count} machines")]
    AmbiguousIdPrefix { prefix: String, count: usize },

    #[error(transparent)]
    StateStore(#[from] rusqlite::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    ImageStore(#[from] bento_runtime::images::store::ImageStoreError),
}
