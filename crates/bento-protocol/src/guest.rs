use serde::{Deserialize, Serialize};

pub const DEFAULT_DISCOVERY_PORT: u32 = 1027;
pub const KERNEL_PARAM_DISCOVERY_PORT: &str = "bento.guest.control_port";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceEndpoint {
    pub name: String,
    pub port: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthStatus {
    pub ok: bool,
}

#[tarpc::service]
pub trait GuestDiscovery {
    async fn list_services() -> Vec<ServiceEndpoint>;
    async fn resolve_service(name: String) -> Option<ServiceEndpoint>;
    async fn health() -> HealthStatus;
}

#[cfg(test)]
mod tests {
    use super::{HealthStatus, ServiceEndpoint};

    #[test]
    fn service_endpoint_round_trips_through_json() {
        let endpoint = ServiceEndpoint {
            name: "ssh".to_string(),
            port: 2022,
        };

        let encoded = serde_json::to_string(&endpoint).expect("serialize endpoint");
        let decoded: ServiceEndpoint =
            serde_json::from_str(&encoded).expect("deserialize endpoint");

        assert_eq!(decoded, endpoint);
    }

    #[test]
    fn health_status_round_trips_through_json() {
        let status = HealthStatus { ok: true };

        let encoded = serde_json::to_string(&status).expect("serialize health status");
        let decoded: HealthStatus =
            serde_json::from_str(&encoded).expect("deserialize health status");

        assert_eq!(decoded, status);
    }
}
