use std::sync::Mutex;

use bento_protocol::v1::{
    CapabilityStatus, EndpointStatus, InspectResponse, LifecycleState, PingResponse, StatusSource,
    StatusUpdate,
};
use tokio::sync::broadcast;

#[derive(Debug)]
pub(crate) struct Bus<E>
where
    E: Clone + Send + 'static,
{
    tx: broadcast::Sender<E>,
}

impl<E> Bus<E>
where
    E: Clone + Send + 'static,
{
    pub(crate) fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<E> {
        self.tx.subscribe()
    }

    pub(crate) fn publish(&self, event: E) {
        let _ = self.tx.send(event);
    }
}

#[derive(Debug)]
pub(crate) struct Store<S, A, E>
where
    E: Clone + Send + 'static,
{
    state: Mutex<S>,
    reducer: fn(&S, &A) -> S,
    projector: fn(&A) -> Option<E>,
    bus: Bus<E>,
}

impl<S, A, E> Store<S, A, E>
where
    E: Clone + Send + 'static,
{
    pub(crate) fn new(
        initial_state: S,
        reducer: fn(&S, &A) -> S,
        projector: fn(&A) -> Option<E>,
        bus_capacity: usize,
    ) -> Self {
        Self {
            state: Mutex::new(initial_state),
            reducer,
            projector,
            bus: Bus::new(bus_capacity),
        }
    }

    pub(crate) fn dispatch(&self, action: A) {
        if let Ok(mut state) = self.state.lock() {
            let next = (self.reducer)(&state, &action);
            *state = next;
        }

        if let Some(event) = (self.projector)(&action) {
            self.bus.publish(event);
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<E> {
        self.bus.subscribe()
    }

    pub(crate) fn snapshot(&self) -> Option<S>
    where
        S: Clone,
    {
        self.state.lock().ok().map(|state| state.clone())
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct InstanceState {
    vm: LifecycleState,
    guest: LifecycleState,
    guest_message: String,
    capabilities: Vec<CapabilityStatus>,
    static_endpoints: Vec<EndpointStatus>,
    dynamic_endpoints: Vec<EndpointStatus>,
}

#[derive(Debug, Clone)]
pub(crate) enum Action {
    VmTransition {
        state: LifecycleState,
        message: String,
    },
    GuestTransition {
        state: LifecycleState,
        message: String,
    },
    SetCapabilities {
        capabilities: Vec<CapabilityStatus>,
    },
    SetStaticEndpoints {
        endpoints: Vec<EndpointStatus>,
    },
    SetDynamicEndpoints {
        endpoints: Vec<EndpointStatus>,
    },
}

impl Action {
    pub(crate) fn vm_starting() -> Self {
        Self::VmTransition {
            state: LifecycleState::Starting,
            message: String::from("vm starting"),
        }
    }

    pub(crate) fn vm_running() -> Self {
        Self::VmTransition {
            state: LifecycleState::Running,
            message: String::from("vm running"),
        }
    }

    pub(crate) fn guest_starting() -> Self {
        Self::GuestTransition {
            state: LifecycleState::Starting,
            message: String::from("waiting for guest capabilities"),
        }
    }

    pub(crate) fn guest_running() -> Self {
        Self::GuestTransition {
            state: LifecycleState::Running,
            message: String::from("startup-required guest capabilities ready"),
        }
    }

    pub(crate) fn guest_error(message: impl Into<String>) -> Self {
        Self::GuestTransition {
            state: LifecycleState::Error,
            message: message.into(),
        }
    }

    pub(crate) fn set_capabilities(capabilities: Vec<CapabilityStatus>) -> Self {
        Self::SetCapabilities { capabilities }
    }

    pub(crate) fn set_static_endpoints(endpoints: Vec<EndpointStatus>) -> Self {
        Self::SetStaticEndpoints { endpoints }
    }

    pub(crate) fn set_dynamic_endpoints(endpoints: Vec<EndpointStatus>) -> Self {
        Self::SetDynamicEndpoints { endpoints }
    }
}

pub(crate) type InstanceStore = Store<InstanceState, Action, StatusUpdate>;

pub(crate) fn new_instance_store() -> InstanceStore {
    Store::new(
        InstanceState::default(),
        reduce_instance_state,
        project_status_update,
        256,
    )
}

pub(crate) fn select_current_ping(state: &InstanceState) -> PingResponse {
    let ok = state.vm == LifecycleState::Running && state.guest == LifecycleState::Running;
    PingResponse {
        ok,
        message: status_summary(state),
    }
}

pub(crate) fn select_current_inspect(state: &InstanceState) -> InspectResponse {
    InspectResponse {
        vm_state: state.vm as i32,
        guest_state: state.guest as i32,
        ready: state.vm == LifecycleState::Running && state.guest == LifecycleState::Running,
        summary: status_summary(state),
        capabilities: state.capabilities.clone(),
        endpoints: state
            .static_endpoints
            .iter()
            .cloned()
            .chain(state.dynamic_endpoints.iter().cloned())
            .collect(),
    }
}

pub(crate) fn select_current_events(state: &InstanceState) -> Vec<StatusUpdate> {
    let mut events = Vec::new();

    if state.vm != LifecycleState::Unspecified {
        events.push(StatusUpdate::new(StatusSource::Vm, state.vm, String::new()));
    }

    if state.guest != LifecycleState::Unspecified {
        events.push(StatusUpdate::new(
            StatusSource::Guest,
            state.guest,
            state.guest_message.clone(),
        ));
    }

    events
}

fn reduce_instance_state(current: &InstanceState, action: &Action) -> InstanceState {
    let mut next = current.clone();

    match action {
        Action::VmTransition { state, .. } => {
            next.vm = *state;
        }
        Action::GuestTransition { state, message } => {
            next.guest = *state;
            next.guest_message = message.clone();
        }
        Action::SetCapabilities { capabilities } => {
            next.capabilities = capabilities.clone();
        }
        Action::SetStaticEndpoints { endpoints } => {
            next.static_endpoints = endpoints.clone();
        }
        Action::SetDynamicEndpoints { endpoints } => {
            next.dynamic_endpoints = endpoints.clone();
        }
    }

    next
}

fn project_status_update(action: &Action) -> Option<StatusUpdate> {
    match action {
        Action::VmTransition { state, message } => {
            Some(StatusUpdate::new(StatusSource::Vm, *state, message.clone()))
        }
        Action::GuestTransition { state, message } => Some(StatusUpdate::new(
            StatusSource::Guest,
            *state,
            message.clone(),
        )),
        Action::SetCapabilities { .. }
        | Action::SetStaticEndpoints { .. }
        | Action::SetDynamicEndpoints { .. } => None,
    }
}

fn status_summary(state: &InstanceState) -> String {
    if state.vm != LifecycleState::Running {
        return format!("vm not ready (vm_state={:?})", state.vm);
    }

    if state.guest == LifecycleState::Running {
        return String::from("instance ready");
    }

    let problems = state
        .capabilities
        .iter()
        .filter(|capability| capability.enabled && capability.startup_required)
        .flat_map(|capability| {
            if capability.configured && capability.running {
                Vec::new()
            } else if capability.problems.is_empty() {
                vec![capability.summary.clone()]
            } else {
                capability.problems.clone()
            }
        })
        .collect::<Vec<_>>();

    if problems.is_empty() {
        state.guest_message.clone()
    } else {
        format!(
            "startup-required capabilities not ready: {}",
            problems.join("; ")
        )
    }
}
