use std::path::PathBuf;
use std::sync::Arc;

use bento_core::InstanceFile;
use bento_vmm::Vmm;
use tokio_util::sync::CancellationToken;

use crate::context::{DaemonContext, VmContext};
use crate::machine::{
    machine_backend_from_vm_spec, machine_identifier_path_from_dir, vm_spec_machine_config,
    VmSpecInputs,
};
use crate::pid_guard::PidGuard;
use crate::runtime::{read_vm_spec_from_dir, remove_stale_socket};
use crate::state::{new_instance_store, Action};

pub async fn init(data_dir: PathBuf) -> eyre::Result<DaemonContext> {
    let vm = load_context(data_dir)?;

    tracing::info!(instance = %vm.name, "vmmon starting");
    remove_stale_socket(&vm.file(InstanceFile::InstancedSocket))?;

    let machine_config = vm_spec_machine_config(VmSpecInputs {
        name: &vm.name,
        data_dir: vm.dir(),
        spec: &vm.spec,
    })?;
    let vmm = Vmm::new(machine_backend_from_vm_spec(&vm.spec)?)?;
    let machine = vmm.create(machine_config.config).await?;
    if let Some(machine_identifier) = machine_config.machine_identifier.as_ref() {
        if machine_identifier.was_generated() {
            let machine_identifier_path = machine_identifier_path_from_dir(vm.dir());
            std::fs::write(machine_identifier_path, machine_identifier.bytes())?;
        }
    }

    let serial_console = machine.serial();
    let store = Arc::new(new_instance_store());
    let guest_enabled = vm.spec.settings.guest_enabled;

    let pid_guard = Arc::new(PidGuard::create(&vm.file(InstanceFile::InstancedPid)).await?);

    store.dispatch(Action::vm_starting());
    machine.start().await?;
    store.dispatch(Action::vm_running());

    Ok(DaemonContext {
        vm,
        machine,
        serial_console,
        store,
        _pid_guard: pid_guard,
        shutdown: CancellationToken::new(),
        guest_enabled,
    })
}

fn load_context(data_dir: PathBuf) -> eyre::Result<VmContext> {
    let spec = read_vm_spec_from_dir(&data_dir)?;
    Ok(VmContext {
        name: spec.name.clone(),
        data_dir,
        spec,
    })
}
