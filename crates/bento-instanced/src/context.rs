use std::path::PathBuf;
use std::sync::Arc;

use bento_protocol::v1::EndpointStatus;
use bento_vmm::{SerialConsole, VirtualMachine};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::monitor_config::VmContext;
use crate::pid_guard::PidGuard;
use crate::service_config::{HostServiceDefinition, ResolvedServiceConfig};
use crate::state::InstanceStore;

pub struct DaemonContext {
    pub(crate) vm: VmContext,
    pub(crate) machine: VirtualMachine,
    pub(crate) serial_console: Arc<SerialConsole>,
    pub(crate) store: Arc<InstanceStore>,
    pub(crate) services: ResolvedServiceConfig,
    pub(crate) host_socket_exports: Vec<HostSocketExport>,
    pub(crate) _pid_guard: PidGuard,
    pub(crate) shutdown: CancellationToken,
    pub(crate) expects_guest_agent: bool,
}

pub struct ServiceHandles {
    pub(crate) control_socket: JoinHandle<eyre::Result<()>>,
    pub(crate) guest_monitor: Option<JoinHandle<()>>,
    pub(crate) serial_log: JoinHandle<()>,
    pub(crate) host_exports: JoinSet<eyre::Result<()>>,
}

#[derive(Clone)]
pub(crate) struct HostSocketExport {
    pub name: String,
    pub host_path: PathBuf,
    pub port: u32,
}

pub(crate) fn configured_host_socket_exports(
    services: &ResolvedServiceConfig,
) -> Vec<HostSocketExport> {
    services
        .host_services
        .iter()
        .filter_map(|service| {
            service
                .host_path
                .as_ref()
                .map(|host_path| HostSocketExport {
                    name: service.id.clone(),
                    host_path: host_path.clone(),
                    port: service.port,
                })
        })
        .collect()
}

pub(crate) fn configured_static_endpoint_statuses(
    services: &[HostServiceDefinition],
) -> Vec<EndpointStatus> {
    crate::guest::project_endpoint_statuses(services, &[])
}
