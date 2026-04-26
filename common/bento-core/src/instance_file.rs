/// Well-known filenames in an instance directory.
pub enum InstanceFile {
    Config,
    VmmonPid,
    VmmonSocket,
    VmmonTraceLog,
    AppleMachineIdentifier,
    SerialLog,
    RootDisk,
    CidataDisk,
}

impl InstanceFile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Config => "config.yaml",
            Self::VmmonPid => "vm.pid",
            Self::VmmonSocket => "vm.sock",
            Self::VmmonTraceLog => "vm.trace.log",
            Self::AppleMachineIdentifier => "apple-machine-id",
            Self::SerialLog => "serial.log",
            Self::RootDisk => "rootfs.img",
            Self::CidataDisk => "cidata.img",
        }
    }
}
