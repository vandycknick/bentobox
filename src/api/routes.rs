use axum::{extract::State, http, Json};
use serde::Deserialize;

use super::AppState;

#[derive(Deserialize)]
pub struct CreateVirtualMachine {
    name: String,
    distro: String,
    cpus: Option<usize>,
    memory: Option<u64>,
}

pub async fn create_vm(
    State(state): State<AppState>,
    Json(payload): Json<CreateVirtualMachine>,
) -> (http::StatusCode, String) {
    let vmm = state.vmm;
    println!("Create vm with name: {}", payload.name);

    let _ = if let Ok(result) = vmm.create() {
        result
    } else {
        return (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "failed".to_string(),
        );
    };

    (http::StatusCode::CREATED, "created".to_string())
}
