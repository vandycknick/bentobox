use serde::{Deserialize, Serialize};

pub mod instance {
    pub mod v1 {
        tonic::include_proto!("instance.v1");
    }
}

pub mod guest {
    pub mod v1 {
        tonic::include_proto!("guest.v1");
    }
}

pub const DEFAULT_DISCOVERY_PORT: u32 = 1027;
pub const KERNEL_PARAM_DISCOVERY_PORT: &str = "bento.guest.control_port";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceEndpoint {
    pub name: String,
    pub port: u32,
}
