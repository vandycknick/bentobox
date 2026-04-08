/// Well-known filenames in an instance directory.
pub enum InstanceFile {
    Config,
    InstancedPid,
    InstancedSocket,
    InstancedTraceLog,
    AppleMachineIdentifier,
    SerialLog,
    RootDisk,
    CidataDisk,
}

impl InstanceFile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Config => "config.yaml",
            Self::InstancedPid => "id.pid",
            Self::InstancedSocket => "id.sock",
            Self::InstancedTraceLog => "id.trace.log",
            Self::AppleMachineIdentifier => "apple-machine-id",
            Self::SerialLog => "serial.log",
            Self::RootDisk => "rootfs.img",
            Self::CidataDisk => "cidata.img",
        }
    }
}
