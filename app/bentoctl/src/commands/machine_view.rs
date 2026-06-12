use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bento_libvm::{MachineInspect, MachineRuntimeStatus, MachineStatus, RequestedNetwork};
use bento_vm_spec::VmSpec;
use serde::Serialize;

use crate::constants::PROFILE_METADATA_KEY;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MachineView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) state: &'static str,
    pub(crate) default: bool,
    pub(crate) profile: Option<String>,
    pub(crate) image: String,
    pub(crate) network: RequestedNetwork,
    pub(crate) created_at: i64,
    pub(crate) modified_at: i64,
    pub(crate) started_at: Option<i64>,
    pub(crate) updated_at: i64,
    pub(crate) root_disk_size: Option<u64>,
    pub(crate) resources: MachineResourcesView,
    pub(crate) process: MachineProcessView,
    pub(crate) guest: MachineGuestView,
    pub(crate) ready: bool,
    pub(crate) summary: Option<String>,
    pub(crate) labels: BTreeMap<String, String>,
    pub(crate) metadata: BTreeMap<String, String>,
    pub(crate) dir: PathBuf,
    pub(crate) spec: VmSpec,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MachineResourcesView {
    pub(crate) cpus: u8,
    pub(crate) memory_mib: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MachineProcessView {
    pub(crate) status: &'static str,
    pub(crate) started_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MachineGuestView {
    pub(crate) status: String,
    pub(crate) ready: bool,
    pub(crate) settings: MachineGuestSettingsView,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MachineGuestSettingsView {
    pub(crate) bootstrap: bool,
    pub(crate) initramfs_present: bool,
}

impl MachineView {
    pub(crate) fn new(
        inspection: &MachineInspect,
        runtime_status: Option<&MachineRuntimeStatus>,
        default: bool,
    ) -> Self {
        let hardware = inspection.spec().hardware.as_ref();
        let guest_status = runtime_status
            .map(|status| status.guest().as_str().to_string())
            .unwrap_or_else(|| "stopped".to_string());
        let summary = runtime_status
            .map(|status| status.summary())
            .filter(|summary| !summary.is_empty())
            .map(str::to_string);

        Self {
            id: inspection.id(),
            name: inspection.name().to_string(),
            state: state_label(inspection.status()),
            default,
            profile: inspection.metadata().get(PROFILE_METADATA_KEY).cloned(),
            image: inspection.image_ref().to_string(),
            network: inspection.network(),
            created_at: inspection.created_at(),
            modified_at: inspection.modified_at(),
            started_at: inspection.started_at(),
            updated_at: inspection.updated_at(),
            root_disk_size: inspection.root_disk_size(),
            resources: MachineResourcesView {
                cpus: hardware.and_then(|hardware| hardware.cpus).unwrap_or(1),
                memory_mib: hardware.and_then(|hardware| hardware.memory).unwrap_or(512),
            },
            process: MachineProcessView {
                status: state_label(inspection.status()),
                started_at: inspection.started_at(),
            },
            guest: MachineGuestView {
                status: guest_status,
                ready: runtime_status.is_some_and(|status| status.guest_ready()),
                settings: guest_settings(inspection.spec(), inspection.instance_dir()),
            },
            ready: runtime_status.is_some_and(|status| status.ready()),
            summary,
            labels: inspection.labels().clone(),
            metadata: inspection.metadata().clone(),
            dir: inspection.instance_dir().to_path_buf(),
            spec: inspection.spec().clone(),
        }
    }
}

pub(crate) fn state_label(state: MachineStatus) -> &'static str {
    match state {
        MachineStatus::Stopped => "stopped",
        MachineStatus::Starting => "starting",
        MachineStatus::Running => "running",
        MachineStatus::Stopping => "stopping",
        MachineStatus::Error => "error",
        _ => "unknown",
    }
}

fn guest_settings(spec: &VmSpec, machine_dir: &Path) -> MachineGuestSettingsView {
    MachineGuestSettingsView {
        bootstrap: spec
            .boot
            .as_ref()
            .and_then(|boot| boot.userdata.as_deref())
            .is_some(),
        initramfs_present: initramfs_path_exists(spec, machine_dir),
    }
}

fn initramfs_path_exists(spec: &VmSpec, machine_dir: &Path) -> bool {
    let Some(initramfs) = spec
        .boot
        .as_ref()
        .and_then(|boot| boot.kernel.as_ref())
        .and_then(|kernel| kernel.initramfs.as_deref())
    else {
        return false;
    };

    if initramfs.is_absolute() {
        initramfs.is_file()
    } else {
        machine_dir.join(initramfs).is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::state_label;
    use bento_libvm::MachineStatus;

    #[test]
    fn labels_machine_states() {
        assert_eq!(state_label(MachineStatus::Stopped), "stopped");
        assert_eq!(state_label(MachineStatus::Starting), "starting");
        assert_eq!(state_label(MachineStatus::Running), "running");
        assert_eq!(state_label(MachineStatus::Stopping), "stopping");
        assert_eq!(state_label(MachineStatus::Error), "error");
    }
}
