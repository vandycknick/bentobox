use bento_protocol::guest::{DEFAULT_DISCOVERY_PORT, KERNEL_PARAM_DISCOVERY_PORT};

pub fn from_kernel_cmdline() -> u32 {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    parse_discovery_port(&cmdline)
}

fn parse_discovery_port(cmdline: &str) -> u32 {
    for token in cmdline.split_whitespace() {
        let key = format!("{}=", KERNEL_PARAM_DISCOVERY_PORT);
        let Some(raw_port) = token.strip_prefix(&key) else {
            continue;
        };

        let Ok(port) = raw_port.parse::<u32>() else {
            continue;
        };

        if (1..=u32::from(u16::MAX)).contains(&port) {
            return port;
        }
    }

    DEFAULT_DISCOVERY_PORT
}

#[cfg(test)]
mod tests {
    use super::parse_discovery_port;
    use bento_protocol::guest::DEFAULT_DISCOVERY_PORT;

    #[test]
    fn parses_control_port_from_kernel_cmdline() {
        assert_eq!(
            parse_discovery_port("root=/dev/vda bento.guest.control_port=7001"),
            7001
        );
    }

    #[test]
    fn falls_back_to_default_when_missing() {
        assert_eq!(
            parse_discovery_port("root=/dev/vda console=hvc0"),
            DEFAULT_DISCOVERY_PORT
        );
    }

    #[test]
    fn falls_back_to_default_on_invalid_value() {
        assert_eq!(
            parse_discovery_port("root=/dev/vda bento.guest.control_port=nope"),
            DEFAULT_DISCOVERY_PORT
        );
    }
}
