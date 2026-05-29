//! `GET /api/usage` — agent plan-usage snapshots for the sidebar chip.
//!
//! Reads `~/.claude/projects` and `~/.codex/sessions` on demand. Both
//! scans are bounded by file mtime + early-exit so a polling client
//! (the dashboard refreshes every 60s) doesn't tax the host.
//!
//! `/api/usage/claude` additionally fetches Anthropic's plan-limit %
//! (spec 001). That fetch is cached for [`CLAUDE_USAGE_TTL`] so N polling
//! clients (TUI + dashboard + PWA) don't multiply upstream OAuth calls —
//! the endpoint is itself rate-limitable, and hitting it from a VPS IP can
//! trip account signals.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::{Json, Router, routing::get};
use tokio::sync::Mutex;

use crate::AppState;
use crate::usage::{ClaudeUsageSnapshot, CodexUsageSnapshot, UsageBundle};

/// Cache TTL for the enriched Claude snapshot. Floors the upstream call
/// cadence regardless of how many clients poll or how low they set their
/// own refresh interval. ≥30s per spec's poll-rate risk mitigation.
const CLAUDE_USAGE_TTL: Duration = Duration::from_secs(30);

/// Process-wide cache for the enriched Claude snapshot. A module-level
/// `OnceLock<Mutex<_>>` rather than an `AppState` field because the data is
/// host-global (not per-connection) and this keeps the change off the shared
/// state struct that every route touches. `tokio::sync::Mutex` because the
/// critical section awaits the OAuth fetch on a cache miss.
fn claude_cache() -> &'static Mutex<Option<(Instant, ClaudeUsageSnapshot)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, ClaudeUsageSnapshot)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/usage", get(get_bundle))
        .route("/api/usage/claude", get(get_claude))
        .route("/api/usage/codex", get(get_codex))
}

// Scans involve synchronous filesystem walking; spawn_blocking keeps
// the tokio worker free for other connections while we crawl.
async fn get_bundle() -> Json<UsageBundle> {
    let snap = tokio::task::spawn_blocking(crate::usage::scan_all)
        .await
        .unwrap_or_else(|_| crate::usage::scan_all());
    Json(snap)
}

async fn get_claude() -> Json<ClaudeUsageSnapshot> {
    let mut guard = claude_cache().lock().await;
    if let Some((at, snap)) = guard.as_ref()
        && at.elapsed() < CLAUDE_USAGE_TTL
    {
        return Json(snap.clone());
    }

    // Cache miss / stale: re-scan transcripts (blocking I/O) then enrich
    // with the OAuth plan-limit fetch (network). Holding the lock across
    // the await serializes concurrent misses so only one upstream call
    // fires per TTL window — exactly the multiplication we want to avoid.
    let scanned = tokio::task::spawn_blocking(crate::usage::scan_claude)
        .await
        .unwrap_or_default();
    let enriched = crate::usage::enrich_claude(scanned).await;
    *guard = Some((Instant::now(), enriched.clone()));
    Json(enriched)
}

async fn get_codex() -> Json<CodexUsageSnapshot> {
    let snap = tokio::task::spawn_blocking(crate::usage::scan_codex)
        .await
        .unwrap_or_default();
    Json(snap)
}
