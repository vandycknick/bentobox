use crate::vm::VirtualMachineBuilder;

#[derive(Debug, Clone)]
pub struct BentoVirtualMachineManager {}

impl BentoVirtualMachineManager {
    pub fn new() -> Self {
        Self {}
    }

    pub fn create(&self) -> eyre::Result<()> {
        let aux_path = "";
        let image_path = "";
        let vm = VirtualMachineBuilder::new()
            .use_cpus(4)
            .use_memory(4294967296)
            .use_platform_macos(aux_path, None)
            .use_storage_device(image_path)
            .use_network()
            .build();

        Ok(())
    }
}
