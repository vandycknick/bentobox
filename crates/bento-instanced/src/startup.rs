use std::path::PathBuf;
use std::sync::Arc;

use bento_core::capabilities::CapabilitiesConfig;
use bento_core::InstanceFile;
use bento_libvm::profiles::{resolve_profiles, validate_capabilities};
use bento_vmm::Vmm;
use tokio_util::sync::CancellationToken;

use crate::bootstrap::rebuild_bootstrap;
use crate::context::{
    configured_host_socket_exports, configured_static_endpoint_statuses, DaemonContext,
};
use crate::machine::{
    machine_backend_from_vm_spec, machine_identifier_path_from_dir, vm_spec_machine_config,
    VmSpecInputs,
};
use crate::monitor_config::VmContext;
use crate::pid_guard::PidGuard;
use crate::runtime::{read_vm_spec_from_dir, remove_stale_socket};
use crate::service_config::resolve_service_config;
use crate::state::{new_instance_store, Action};

pub async fn init(data_dir: PathBuf, profiles: Vec<String>) -> eyre::Result<DaemonContext> {
    let vm = load_context(data_dir, &profiles)?;
    let resolved_capabilities = resolve_startup_capabilities(&vm, &profiles)?;
    let services = resolve_service_config(&vm, &resolved_capabilities)?;

    tracing::info!(instance = %vm.name, "vmmon starting");
    rebuild_bootstrap(&vm, &services.guest)?;
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
    let expects_guest_agent = vm.requires_bootstrap_for(&services.guest);
    let host_socket_exports = configured_host_socket_exports(&services);
    for export in &host_socket_exports {
        remove_stale_socket(&export.host_path)?;
    }

    let configured_endpoints = configured_static_endpoint_statuses(&services.host_services);
    store.dispatch(Action::set_static_endpoints(configured_endpoints));
    store.dispatch(Action::set_dynamic_endpoints(Vec::new()));

    let pid_guard = PidGuard::create(&vm.file(InstanceFile::InstancedPid)).await?;

    Ok(DaemonContext {
        vm,
        machine,
        serial_console,
        store,
        services,
        host_socket_exports,
        _pid_guard: pid_guard,
        shutdown: CancellationToken::new(),
        expects_guest_agent,
    })
}

fn load_context(data_dir: PathBuf, _profiles: &[String]) -> eyre::Result<VmContext> {
    let spec = read_vm_spec_from_dir(&data_dir)?;
    Ok(VmContext {
        name: spec.name.clone(),
        data_dir,
        spec,
    })
}

fn resolve_startup_capabilities(
    context: &VmContext,
    start_profiles: &[String],
) -> eyre::Result<CapabilitiesConfig> {
    let mut all_profiles = context.profiles().to_vec();
    all_profiles.extend(start_profiles.iter().cloned());
    let base = context.base_capabilities();
    let capabilities = resolve_profiles(&base, &all_profiles)?;
    validate_capabilities(&capabilities)?;
    Ok(capabilities)
}
