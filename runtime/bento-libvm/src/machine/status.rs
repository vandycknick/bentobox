use bento_protocol::v1::{InspectResponse, LifecycleState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineRuntimeStatus {
    vm: RuntimeComponentStatus,
    guest: RuntimeComponentStatus,
    ready: bool,
    summary: String,
}

impl MachineRuntimeStatus {
    pub(crate) fn from_protocol(response: InspectResponse) -> Self {
        Self {
            vm: RuntimeComponentStatus::from_raw(response.vm_state),
            guest: RuntimeComponentStatus::from_raw(response.guest_state),
            ready: response.ready,
            summary: response.summary,
        }
    }

    pub fn vm(&self) -> RuntimeComponentStatus {
        self.vm
    }

    pub fn guest(&self) -> RuntimeComponentStatus {
        self.guest
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn guest_ready(&self) -> bool {
        self.guest.is_running() && self.ready
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeComponentStatus {
    Unspecified,
    Starting,
    Running,
    Stopping,
    Stopped,
    Error,
}

impl RuntimeComponentStatus {
    fn from_raw(raw: i32) -> Self {
        match LifecycleState::try_from(raw).unwrap_or(LifecycleState::Unspecified) {
            LifecycleState::Unspecified => Self::Unspecified,
            LifecycleState::Starting => Self::Starting,
            LifecycleState::Running => Self::Running,
            LifecycleState::Stopping => Self::Stopping,
            LifecycleState::Stopped => Self::Stopped,
            LifecycleState::Error => Self::Error,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
}

#[cfg(test)]
mod tests {
    use bento_protocol::v1::LifecycleState;

    use super::{MachineRuntimeStatus, RuntimeComponentStatus};

    #[test]
    fn converts_protocol_status_to_public_view() {
        let status = MachineRuntimeStatus::from_protocol(bento_protocol::v1::InspectResponse {
            vm_state: LifecycleState::Running as i32,
            guest_state: LifecycleState::Starting as i32,
            ready: false,
            summary: "booting".to_string(),
        });

        assert_eq!(status.vm(), RuntimeComponentStatus::Running);
        assert_eq!(status.guest(), RuntimeComponentStatus::Starting);
        assert!(!status.ready());
        assert_eq!(status.summary(), "booting");
    }
}
