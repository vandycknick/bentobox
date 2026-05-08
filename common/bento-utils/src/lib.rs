pub fn format_mac(mac: [u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

#[cfg(test)]
mod tests {
    use crate::format_mac;

    #[test]
    fn formats_mac_as_lowercase_colon_hex() {
        assert_eq!(
            format_mac([0x02, 0x94, 0xef, 0xe4, 0x0c, 0xee]),
            "02:94:ef:e4:0c:ee"
        );
    }
}
