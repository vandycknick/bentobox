use crate::network::RequestedNetwork;

#[derive(Debug, Clone, Default)]
pub struct MachineUpdate {
    pub name: Option<String>,
    pub cpus: Option<u8>,
    pub memory_mib: Option<u32>,
    pub root_disk_size: Option<u64>,
    pub nested_virtualization: Option<bool>,
    pub rosetta: Option<bool>,
    pub network: Option<RequestedNetwork>,
}

impl MachineUpdate {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.cpus.is_none()
            && self.memory_mib.is_none()
            && self.root_disk_size.is_none()
            && self.nested_virtualization.is_none()
            && self.rosetta.is_none()
            && self.network.is_none()
    }
}
