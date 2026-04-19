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

pub const DEFAULT_AGENT_CONTROL_PORT: u32 = 1027;
pub const KERNEL_PARAM_AGENT_PORT: &str = "bento.guest.port";

pub fn agent_port_arg(port: u32) -> String {
    format!("{}={port}", KERNEL_PARAM_AGENT_PORT)
}

pub fn parse_agent_port_args<'a>(args: impl IntoIterator<Item = &'a str>) -> u32 {
    for arg in args {
        let Some(raw_port) = arg.strip_prefix(&format!("{}=", KERNEL_PARAM_AGENT_PORT)) else {
            continue;
        };

        let Ok(port) = raw_port.parse::<u32>() else {
            continue;
        };

        if (1..=u32::from(u16::MAX)).contains(&port) {
            return port;
        }
    }

    DEFAULT_AGENT_CONTROL_PORT
}
