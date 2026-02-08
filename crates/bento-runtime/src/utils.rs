use std::{fs, io, num::NonZeroI32, path::Path};

/// Read PID file and verify liveness.
///
/// Returns:
/// - `Ok(None)` if file is missing OR PID is stale (stale file is removed)
/// - `Ok(Some(pid))` if process is considered alive
/// - `Err(...)` on unexpected I/O or parsing errors
pub fn read_pid_file(path: &Path) -> io::Result<Option<NonZeroI32>> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    let pid: i32 = raw.trim().parse().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid pid in {}: {err}", path.display()),
        )
    })?;

    let pid = NonZeroI32::new(pid).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("pid in {} must be non-zero", path.display()),
        )
    })?;

    // SAFETY: libc::kill is called with primitive integer arguments.
    let rc = unsafe { libc::kill(pid.get(), 0) };
    if rc == 0 {
        return Ok(Some(pid));
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::ESRCH => {
            // Process does not exist, clean stale pid file.
            let _ = fs::remove_file(path);
            Ok(None)
        }
        Some(code) if code == libc::EPERM => {
            // No permission to signal, process still exists.
            Ok(Some(pid))
        }
        _ => Err(err),
    }
}
