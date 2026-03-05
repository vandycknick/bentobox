use std::sync::Mutex;

use bento_protocol::instance::v1::{HealthResponse, LifecycleState, StatusSource, StatusUpdate};
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

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InstanceState {
    vm: LifecycleState,
    guest: LifecycleState,
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
            message: String::from("guest services ready"),
        }
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
    let ok = state.guest == LifecycleState::Running;
    HealthResponse {
        ok,
        message: if ok {
            String::new()
        } else {
            format!(
                "guest not ready (vm_state={:?}, guest_state={:?})",
                state.vm, state.guest
            )
        },
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
            String::new(),
        ));
    }

    events
}

fn reduce_instance_state(current: &InstanceState, action: &Action) -> InstanceState {
    let mut next = *current;

    match action {
        Action::VmTransition { state, .. } => {
            next.vm = *state;
        }
        Action::GuestTransition { state, .. } => {
            next.guest = *state;
        }
    }

    next
}

fn project_status_update(action: &Action) -> Option<StatusUpdate> {
    let update = match action {
        Action::VmTransition { state, message } => {
            StatusUpdate::new(StatusSource::Vm, *state, message.clone())
        }
        Action::GuestTransition { state, message } => {
            StatusUpdate::new(StatusSource::Guest, *state, message.clone())
        }
    };

    Some(update)
}
