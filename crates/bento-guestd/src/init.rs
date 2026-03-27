pub fn ensure_supported_runtime_mode() {
    if std::process::id() == 1 {
        panic!("bento-guestd PID 1 mode is not implemented yet");
    }
}
