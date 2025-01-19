pub fn size_in_bytes(size_str: &str) -> u64 {
    let (number, unit) = if let Some(index) = size_str.find(|c: char| !c.is_digit(10)) {
        let number_part = &size_str[..index];
        let unit_part = &size_str[index..];
        (number_part, unit_part)
    } else {
        ("", "")
    };

    if number == "" {
        return 0;
    }

    let number: u64 = number.parse().unwrap_or(0);
    let bytes = match unit.to_lowercase().as_str() {
        "gb" => number * 1024 * 1024 * 1024, // 1 GB = 1024^3 bytes
        "mb" => number * 1024 * 1024,        // 1 MB = 1024^2 bytes
        "kb" => number * 1024,               // 1 KB = 1024 bytes
        _ => number,                         // Return None for unrecognized units
    };

    bytes
}
