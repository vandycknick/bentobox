use std::{fs, io, num::NonZeroI32, path::Path};

use libc::pid_t;
use nix::errno::Errno;
use nix::sys::signal;
use nix::unistd::Pid;

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

    let pid: pid_t = raw.trim().parse().map_err(|err| {
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

    match signal::kill(Pid::from_raw(pid.get()), None) {
        Ok(()) => Ok(Some(pid)),
        Err(Errno::ESRCH) => {
            // Process does not exist, clean stale pid file.
            let _ = fs::remove_file(path);
            Ok(None)
        }
        Err(Errno::EPERM) => {
            // No permission to signal, process still exists.
            Ok(Some(pid))
        }
        Err(errno) => Err(io::Error::from_raw_os_error(errno as i32)),
    }
}
