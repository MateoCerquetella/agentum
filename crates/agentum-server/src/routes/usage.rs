//! `GET /api/usage` — agent plan-usage snapshots for the sidebar chip.
//!
//! Reads `~/.claude/projects` and `~/.codex/sessions` on demand. Both
//! scans are bounded by file mtime + early-exit so a polling client
//! (the dashboard refreshes every 60s) doesn't tax the host.

use axum::{Json, Router, routing::get};

use crate::AppState;
use crate::usage::{ClaudeUsageSnapshot, CodexUsageSnapshot, UsageBundle};

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
    let snap = tokio::task::spawn_blocking(crate::usage::scan_claude)
        .await
        .unwrap_or_default();
    Json(snap)
}

async fn get_codex() -> Json<CodexUsageSnapshot> {
    let snap = tokio::task::spawn_blocking(crate::usage::scan_codex)
        .await
        .unwrap_or_default();
    Json(snap)
}
