pub mod negotiate;
pub mod services;

pub mod v1 {
    tonic::include_proto!("bento.v1");

    impl StatusUpdate {
        pub fn new(
            source: StatusSource,
            state: LifecycleState,
            message: impl Into<String>,
        ) -> Self {
            Self {
                source: source as i32,
                state: state as i32,
                message: message.into(),
                timestamp_unix_ms: unix_time_ms(),
            }
        }
    }

    fn unix_time_ms() -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as i64,
            Err(_) => 0,
        }
    }
}

pub const DEFAULT_DISCOVERY_PORT: u32 = 1027;
pub const KERNEL_PARAM_DISCOVERY_PORT: &str = "bento.guest.control_port";
