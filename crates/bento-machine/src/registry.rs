use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crossbeam::channel;
use tokio::sync::{mpsc, oneshot};

use crate::backend::create_backend;
use crate::types::{
    MachineError, MachineId, MachineState, OpenDeviceRequest, OpenDeviceResponse,
    ResolvedMachineSpec,
};

pub(crate) struct MachineInner {
    pub(crate) spec: ResolvedMachineSpec,
    released: AtomicBool,
    tx: mpsc::UnboundedSender<MachineCommand>,
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
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MachineCommand::State { reply: reply_tx })
            .map_err(|_| MachineError::Backend("machine worker has stopped".to_string()))?;
        reply_rx
            .await
            .map_err(|_| MachineError::Backend("machine worker dropped reply".to_string()))?
    }

    pub(crate) async fn start(&self) -> Result<(), MachineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MachineCommand::Start { reply: reply_tx })
            .map_err(|_| MachineError::Backend("machine worker has stopped".to_string()))?;
        reply_rx
            .await
            .map_err(|_| MachineError::Backend("machine worker dropped reply".to_string()))?
    }

    pub(crate) async fn stop(&self) -> Result<(), MachineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MachineCommand::Stop { reply: reply_tx })
            .map_err(|_| MachineError::Backend("machine worker has stopped".to_string()))?;
        reply_rx
            .await
            .map_err(|_| MachineError::Backend("machine worker dropped reply".to_string()))?
    }

    pub(crate) async fn open_device(
        &self,
        request: OpenDeviceRequest,
    ) -> Result<OpenDeviceResponse, MachineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MachineCommand::OpenDevice {
                request,
                reply: reply_tx,
            })
            .map_err(|_| MachineError::Backend("machine worker has stopped".to_string()))?;
        reply_rx
            .await
            .map_err(|_| MachineError::Backend("machine worker dropped reply".to_string()))?
    }
}

enum MachineCommand {
    State {
        reply: oneshot::Sender<Result<MachineState, MachineError>>,
    },
    Start {
        reply: oneshot::Sender<Result<(), MachineError>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), MachineError>>,
    },
    OpenDevice {
        request: OpenDeviceRequest,
        reply: oneshot::Sender<Result<OpenDeviceResponse, MachineError>>,
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
) -> Result<mpsc::UnboundedSender<MachineCommand>, MachineError> {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let (startup_tx, startup_rx) = channel::bounded(1);

    thread::Builder::new()
        .name(format!("machine:{}", spec.id.as_str()))
        .spawn(move || {
            let backend = create_backend(&spec);
            match backend {
                Ok(mut backend) => {
                    let _ = startup_tx.send(Ok(()));
                    while let Some(command) = command_rx.blocking_recv() {
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
