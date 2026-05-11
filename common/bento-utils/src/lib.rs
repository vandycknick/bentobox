pub fn format_mac(mac: [u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

pub fn parse_mac(input: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = input.split(':').collect();
    if parts.len() != 6 {
        return Err("expected MAC as xx:xx:xx:xx:xx:xx".to_string());
    }

    let mut mac = [0; 6];
    for (index, part) in parts.iter().enumerate() {
        if part.len() != 2 {
            return Err(format!(
                "invalid MAC byte {part:?}: expected two hex digits"
            ));
        }
        mac[index] = u8::from_str_radix(part, 16)
            .map_err(|err| format!("invalid MAC byte {part:?}: {err}"))?;
    }

    if mac[0] & 0x01 != 0 {
        return Err("MAC address cannot be multicast".to_string());
    }

    Ok(mac)
}

#[cfg(test)]
mod tests {
    use crate::{format_mac, parse_mac};

    #[test]
    fn formats_mac_as_lowercase_colon_hex() {
        assert_eq!(
            format_mac([0x02, 0x94, 0xef, 0xe4, 0x0c, 0xee]),
            "02:94:ef:e4:0c:ee"
        );
    }

    #[test]
    fn parses_colon_hex_mac() {
        assert_eq!(
            parse_mac("02:94:ef:e4:0c:ee").expect("parse mac"),
            [0x02, 0x94, 0xef, 0xe4, 0x0c, 0xee]
        );
    }

    #[test]
    fn rejects_multicast_mac() {
        assert!(parse_mac("03:94:ef:e4:0c:ee").is_err());
    }
}
