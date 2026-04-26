use std::sync::Mutex;

use bento_protocol::v1::{
    EndpointStatus, InspectResponse, LifecycleState, PingResponse, ServiceHealth, StatusSource,
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
    services: Vec<ServiceHealth>,
    endpoints: Vec<EndpointStatus>,
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
    SetServices {
        services: Vec<ServiceHealth>,
    },
    UpsertEndpoint {
        endpoint: EndpointStatus,
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
            message: String::from("waiting for guest services"),
        }
    }

    pub(crate) fn guest_running() -> Self {
        Self::GuestTransition {
            state: LifecycleState::Running,
            message: String::from("startup-required guest services ready"),
        }
    }

    pub(crate) fn guest_error(message: impl Into<String>) -> Self {
        Self::GuestTransition {
            state: LifecycleState::Error,
            message: message.into(),
        }
    }

    pub(crate) fn set_services(services: Vec<ServiceHealth>) -> Self {
        Self::SetServices { services }
    }

    pub(crate) fn upsert_endpoint(endpoint: EndpointStatus) -> Self {
        Self::UpsertEndpoint { endpoint }
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
        services: state.services.clone(),
        endpoints: state.endpoints.clone(),
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

pub(crate) fn guest_shell_ready(state: &InstanceState) -> bool {
    state.guest == LifecycleState::Running
        && state
            .services
            .iter()
            .find(|service| service.name == "shell")
            .map(|service| service.healthy)
            .unwrap_or(false)
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
        Action::SetServices { services } => {
            next.services = services.clone();
        }
        Action::UpsertEndpoint { endpoint } => {
            if let Some(existing) = next
                .endpoints
                .iter_mut()
                .find(|item| item.name == endpoint.name)
            {
                *existing = endpoint.clone();
            } else {
                next.endpoints.push(endpoint.clone());
                next.endpoints
                    .sort_by(|left, right| left.name.cmp(&right.name));
            }
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
        Action::SetServices { .. } | Action::UpsertEndpoint { .. } => None,
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
        .services
        .iter()
        .filter(|service| service.startup_required)
        .flat_map(|service| {
            if service.healthy {
                Vec::new()
            } else if service.problems.is_empty() {
                vec![service.summary.clone()]
            } else {
                service.problems.clone()
            }
        })
        .collect::<Vec<_>>();

    if problems.is_empty() {
        state.guest_message.clone()
    } else {
        format!(
            "startup-required services not ready: {}",
            problems.join("; ")
        )
    }
}
