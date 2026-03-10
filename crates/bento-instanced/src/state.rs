use std::sync::Mutex;

use bento_protocol::instance::v1::{
    ExtensionStatus, GetStatusResponse, HealthResponse, HostSocket, LifecycleState,
    PortForwardStatus, StatusSource, StatusUpdate,
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
    extensions: Vec<ExtensionStatus>,
    host_sockets: Vec<HostSocket>,
    port_forwards: Vec<PortForwardStatus>,
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
    SetExtensions {
        extensions: Vec<ExtensionStatus>,
    },
    SetHostSockets {
        host_sockets: Vec<HostSocket>,
    },
    SetPortForwards {
        port_forwards: Vec<PortForwardStatus>,
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
            message: String::from("waiting for guest extensions"),
        }
    }

    pub(crate) fn guest_running() -> Self {
        Self::GuestTransition {
            state: LifecycleState::Running,
            message: String::from("startup-required guest extensions ready"),
        }
    }

    pub(crate) fn guest_error(message: impl Into<String>) -> Self {
        Self::GuestTransition {
            state: LifecycleState::Error,
            message: message.into(),
        }
    }

    pub(crate) fn set_extensions(extensions: Vec<ExtensionStatus>) -> Self {
        Self::SetExtensions { extensions }
    }

    pub(crate) fn set_host_sockets(host_sockets: Vec<HostSocket>) -> Self {
        Self::SetHostSockets { host_sockets }
    }

    pub(crate) fn set_port_forwards(port_forwards: Vec<PortForwardStatus>) -> Self {
        Self::SetPortForwards { port_forwards }
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

pub(crate) fn select_current_health(state: &InstanceState) -> HealthResponse {
    let ok = state.vm == LifecycleState::Running && state.guest == LifecycleState::Running;
    HealthResponse {
        ok,
        message: status_summary(state),
    }
}

pub(crate) fn select_current_status(state: &InstanceState) -> GetStatusResponse {
    GetStatusResponse {
        vm_state: state.vm as i32,
        guest_state: state.guest as i32,
        ready: state.vm == LifecycleState::Running && state.guest == LifecycleState::Running,
        summary: status_summary(state),
        extensions: state.extensions.clone(),
        host_sockets: state.host_sockets.clone(),
        port_forwards: state.port_forwards.clone(),
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
        Action::SetExtensions { extensions } => {
            next.extensions = extensions.clone();
        }
        Action::SetHostSockets { host_sockets } => {
            next.host_sockets = host_sockets.clone();
        }
        Action::SetPortForwards { port_forwards } => {
            next.port_forwards = port_forwards.clone();
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
        Action::SetExtensions { .. }
        | Action::SetHostSockets { .. }
        | Action::SetPortForwards { .. } => None,
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
        .extensions
        .iter()
        .filter(|extension| extension.enabled && extension.startup_required)
        .flat_map(|extension| {
            if extension.configured && extension.running {
                Vec::new()
            } else if extension.problems.is_empty() {
                vec![extension.summary.clone()]
            } else {
                extension.problems.clone()
            }
        })
        .collect::<Vec<_>>();

    if problems.is_empty() {
        state.guest_message.clone()
    } else {
        format!(
            "startup-required extensions not ready: {}",
            problems.join("; ")
        )
    }
}
