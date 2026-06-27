//! Wall-clock timestamp helpers (milliseconds since the Unix epoch).

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch as `u64` (0 on a clock error).
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

/// Milliseconds since the Unix epoch as `i64`, for SQLite / serde sinks (0 on a clock error).
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
