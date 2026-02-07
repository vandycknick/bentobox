use std::sync::Arc;

use axum::{routing::post, Router};
use routes::create_vm;

use crate::core::bento_vmm::BentoVirtualMachineManager;

mod models;
mod routes;

#[derive(Debug, Clone)]
pub struct AppState {
    vmm: Arc<BentoVirtualMachineManager>,
}

pub fn create_router() -> Router {
    let state = AppState {
        vmm: Arc::new(BentoVirtualMachineManager::new()),
    };

    Router::new()
        .route("/vm", post(create_vm))
        .with_state(state)
}
