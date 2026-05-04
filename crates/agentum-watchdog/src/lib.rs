//! Per-session watchdog. Real implementation in PRD phase 6.

#[derive(Debug, thiserror::Error)]
pub enum WatchdogError {
    #[error("watchdog not yet implemented (phase 6)")]
    NotImplemented,
}
