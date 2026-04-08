use std::path::PathBuf;

use bento_core::capabilities::CapabilitiesConfig;
use bento_core::services::{
    GuestRuntimeConfig, GuestServiceConfig, GuestServiceKind, RESERVED_FORWARD_PORT_START,
    RESERVED_SSH_PORT, SERVICE_ID_SSH,
};

use crate::monitor_config::VmContext;

#[derive(Debug, Clone)]
pub(crate) struct HostServiceDefinition {
    pub id: String,
    pub kind: GuestServiceKind,
    pub port: u32,
    pub target: String,
    pub host_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedServiceConfig {
    pub guest: GuestRuntimeConfig,
    pub host_services: Vec<HostServiceDefinition>,
}

pub(crate) fn resolve_service_config(
    context: &VmContext,
    capabilities: &CapabilitiesConfig,
) -> eyre::Result<ResolvedServiceConfig> {
    if capabilities.forward.tcp.auto_discover {
        return Err(eyre::eyre!(
            "forward.tcp.auto_discover is not supported with static vmmon service ports"
        ));
    }

    let mut guest_services = Vec::new();
    let mut host_services = Vec::new();

    if capabilities.ssh.enabled {
        guest_services.push(GuestServiceConfig {
            id: SERVICE_ID_SSH.to_string(),
            kind: GuestServiceKind::Ssh,
            port: RESERVED_SSH_PORT,
            startup_required: true,
            target: String::from("127.0.0.1:22"),
        });
        host_services.push(HostServiceDefinition {
            id: SERVICE_ID_SSH.to_string(),
            kind: GuestServiceKind::Ssh,
            port: RESERVED_SSH_PORT,
            target: String::from("127.0.0.1:22"),
            host_path: None,
        });
    }

    for (index, forward) in capabilities.forward.uds.iter().enumerate() {
        let port = RESERVED_FORWARD_PORT_START + index as u32;
        guest_services.push(GuestServiceConfig {
            id: forward.name.clone(),
            kind: GuestServiceKind::UnixSocketForward,
            port,
            startup_required: false,
            target: forward.guest_path.clone(),
        });
        host_services.push(HostServiceDefinition {
            id: forward.name.clone(),
            kind: GuestServiceKind::UnixSocketForward,
            port,
            target: forward.guest_path.clone(),
            host_path: Some(context.dir().join("sock").join(&forward.host_path)),
        });
    }

    Ok(ResolvedServiceConfig {
        guest: GuestRuntimeConfig {
            dns: capabilities.dns.clone(),
            services: guest_services,
            ..GuestRuntimeConfig::default()
        },
        host_services,
    })
}
