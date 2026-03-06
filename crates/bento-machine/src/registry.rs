use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crossbeam::channel;

use crate::backend::create_backend;
use crate::types::{
    MachineError, MachineId, MachineState, OpenDeviceRequest, OpenDeviceResponse,
    ResolvedMachineSpec,
};

pub(crate) struct MachineInner {
    pub(crate) spec: ResolvedMachineSpec,
    released: AtomicBool,
    tx: channel::Sender<MachineCommand>,
}

impl MachineInner {
    pub(crate) fn new(spec: ResolvedMachineSpec) -> Result<Self, MachineError> {
        let tx = spawn_machine_worker(spec.clone())?;
        Ok(Self {
            spec,
            released: AtomicBool::new(false),
            tx,
        })
    }

    pub(crate) fn is_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_released(&self) {
        self.released.store(true, Ordering::SeqCst);
    }

    pub(crate) async fn state(&self) -> Result<MachineState, MachineError> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            send_command(&tx, |reply| MachineCommand::State { reply })
        })
        .await
        .map_err(|_| MachineError::Backend("machine state task failed to join".to_string()))?
    }

    pub(crate) async fn start(&self) -> Result<(), MachineError> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            send_command(&tx, |reply| MachineCommand::Start { reply })
        })
        .await
        .map_err(|_| MachineError::Backend("machine start task failed to join".to_string()))?
    }

    pub(crate) async fn stop(&self) -> Result<(), MachineError> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            send_command(&tx, |reply| MachineCommand::Stop { reply })
        })
        .await
        .map_err(|_| MachineError::Backend("machine stop task failed to join".to_string()))?
    }

    pub(crate) async fn open_device(
        &self,
        request: OpenDeviceRequest,
    ) -> Result<OpenDeviceResponse, MachineError> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            send_command(&tx, |reply| MachineCommand::OpenDevice { request, reply })
        })
        .await
        .map_err(|_| MachineError::Backend("open_device task failed to join".to_string()))?
    }
}

enum MachineCommand {
    State {
        reply: channel::Sender<Result<MachineState, MachineError>>,
    },
    Start {
        reply: channel::Sender<Result<(), MachineError>>,
    },
    Stop {
        reply: channel::Sender<Result<(), MachineError>>,
    },
    OpenDevice {
        request: OpenDeviceRequest,
        reply: channel::Sender<Result<OpenDeviceResponse, MachineError>>,
    },
}

#[derive(Default)]
struct Registry {
    machines: HashMap<MachineId, Arc<MachineInner>>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

pub(crate) fn create_or_get(spec: ResolvedMachineSpec) -> Result<Arc<MachineInner>, MachineError> {
    let mut registry = registry()
        .lock()
        .map_err(|_| MachineError::RegistryPoisoned)?;

    if let Some(existing) = registry.machines.get(&spec.id) {
        if existing.spec == spec {
            return Ok(existing.clone());
        }

        return Err(MachineError::SpecMismatch {
            id: spec.id.clone(),
            existing: Box::new(existing.spec.clone()),
            requested: Box::new(spec),
        });
    }

    let id = spec.id.clone();
    let machine = Arc::new(MachineInner::new(spec)?);
    registry.machines.insert(id, machine.clone());
    Ok(machine)
}

pub(crate) fn release(id: &MachineId) -> Result<Option<Arc<MachineInner>>, MachineError> {
    let mut registry = registry()
        .lock()
        .map_err(|_| MachineError::RegistryPoisoned)?;
    let machine = registry.machines.remove(id);
    if let Some(machine) = machine.as_ref() {
        machine.mark_released();
    }
    Ok(machine)
}

fn spawn_machine_worker(
    spec: ResolvedMachineSpec,
) -> Result<channel::Sender<MachineCommand>, MachineError> {
    let (command_tx, command_rx) = channel::unbounded();
    let (startup_tx, startup_rx) = channel::bounded(1);

    thread::Builder::new()
        .name(format!("machine:{}", spec.id.as_str()))
        .spawn(move || {
            let backend = create_backend(&spec);
            match backend {
                Ok(mut backend) => {
                    let _ = startup_tx.send(Ok(()));
                    while let Ok(command) = command_rx.recv() {
                        match command {
                            MachineCommand::State { reply } => {
                                let _ = reply.send(backend.state());
                            }
                            MachineCommand::Start { reply } => {
                                let _ = reply.send(backend.start());
                            }
                            MachineCommand::Stop { reply } => {
                                let result = backend.stop();
                                let _ = reply.send(result);
                                break;
                            }
                            MachineCommand::OpenDevice { request, reply } => {
                                let _ = reply.send(backend.open_device(request));
                            }
                        }
                    }
                }
                Err(err) => {
                    let _ = startup_tx.send(Err(err));
                }
            }
        })
        .map_err(|err| MachineError::Backend(format!("spawn machine worker failed: {err}")))?;

    startup_rx.recv().map_err(|_| {
        MachineError::Backend("machine worker failed before initialization".to_string())
    })??;

    Ok(command_tx)
}

fn send_command<T>(
    tx: &channel::Sender<MachineCommand>,
    build: impl FnOnce(channel::Sender<Result<T, MachineError>>) -> MachineCommand,
) -> Result<T, MachineError> {
    let (reply_tx, reply_rx) = channel::bounded(1);
    tx.send(build(reply_tx))
        .map_err(|_| MachineError::Backend("machine worker has stopped".to_string()))?;
    reply_rx
        .recv()
        .map_err(|_| MachineError::Backend("machine worker dropped reply".to_string()))?
}
