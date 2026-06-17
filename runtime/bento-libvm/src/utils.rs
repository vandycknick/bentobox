use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current Unix timestamp in seconds.
///
/// libvm stores persistence timestamps as signed SQLite integers. If the host
/// clock is before the Unix epoch, this returns `0` instead of panicking. The
/// result is also clamped to `i64::MAX` before conversion so callers can bind it
/// directly into integer timestamp columns.
pub(crate) fn now_unix() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    seconds.min(i64::MAX as u64) as i64
}
