use agentum_server::usage::{
    enrich_claude, scan_claude, scan_codex, ClaudeUsageSnapshot, CodexUsageSnapshot,
    CodexUsageWindow,
};
use serde_json::{json, Value};
use super::timestamps::now_ms;

// Rate-limit tracking maps the embedded server's on-disk usage scan
// (`agentum_server::usage`) onto the renderer's `RateLimitState` shape
// (see ui/shared/rate-limit-types.ts). Claude reads `~/.claude` transcripts and
// enriches with `/api/oauth/usage`; Codex reads the rate-limit block from the
// newest `~/.codex/sessions` record. A provider the host never installed maps to
// `null` so the status bar hides its segment rather than showing a stub.

const SESSION_WINDOW_MINUTES: u32 = 300; // 5h rolling window
const WEEKLY_WINDOW_MINUTES: u32 = 10_080; // 7d rolling window

/// One `RateLimitWindow` (`{ usedPercent, windowMinutes, resetsAt, resetDescription }`).
fn window(used_percent: f64, window_minutes: u32, resets_at_ms: Option<i64>) -> Value {
    json!({
        "usedPercent": used_percent.clamp(0.0, 100.0),
        "windowMinutes": window_minutes,
        "resetsAt": resets_at_ms,
        // The renderer derives its own "resets in …" label from resetsAt.
        "resetDescription": Value::Null,
    })
}

/// Map a Claude usage snapshot onto `ProviderRateLimits | null`.
///
/// `null` when Claude was never run on this host (segment hidden). When the
/// OAuth fetch succeeded we emit `status: "ok"` with real 5h/7d windows; when it
/// didn't (no token / fetch failed) we emit `status: "error"` so the bar shows a
/// "sign in" affordance instead of a fabricated percentage.
fn claude_provider(snap: &ClaudeUsageSnapshot) -> Value {
    if !snap.claude_installed {
        return Value::Null;
    }

    // The snapshot keeps a single reset time for whichever window is the binding
    // constraint (`max(5h, 7d)`); attach it to that window, leave the other null.
    let five_is_binding = match (snap.five_hour_pct, snap.seven_day_pct) {
        (Some(a), Some(b)) => a >= b,
        (Some(_), None) => true,
        _ => false,
    };
    let session = snap.five_hour_pct.map(|pct| {
        let resets = if five_is_binding {
            snap.resets_at_ms
        } else {
            None
        };
        window(pct, SESSION_WINDOW_MINUTES, resets)
    });
    let weekly = snap.seven_day_pct.map(|pct| {
        let resets = if five_is_binding {
            None
        } else {
            snap.resets_at_ms
        };
        window(pct, WEEKLY_WINDOW_MINUTES, resets)
    });

    let has_usage = session.is_some() || weekly.is_some();
    json!({
        "provider": "claude",
        "session": session,
        "weekly": weekly,
        "updatedAt": now_ms(),
        "error": if has_usage { Value::Null } else { json!("Sign in to Claude to see usage") },
        "status": if has_usage { "ok" } else { "error" },
    })
}

/// Map one Codex rate-limit bucket onto a `RateLimitWindow`.
fn codex_window(w: &CodexUsageWindow, fallback_minutes: u32) -> Value {
    let minutes = if w.window_minutes > 0 {
        w.window_minutes
    } else {
        fallback_minutes
    };
    // Codex reports resets as unix *seconds*; the UI expects unix ms.
    let resets = (w.resets_at > 0).then_some(w.resets_at * 1000);
    window(w.used_percent, minutes, resets)
}

/// Map a Codex usage snapshot onto `ProviderRateLimits | null`.
fn codex_provider(snap: &CodexUsageSnapshot) -> Value {
    if !snap.codex_installed {
        return Value::Null;
    }

    let session = snap
        .primary
        .as_ref()
        .map(|w| codex_window(w, SESSION_WINDOW_MINUTES));
    let weekly = snap
        .secondary
        .as_ref()
        .map(|w| codex_window(w, WEEKLY_WINDOW_MINUTES));

    let has_usage = session.is_some() || weekly.is_some();
    json!({
        "provider": "codex",
        "session": session,
        "weekly": weekly,
        "updatedAt": now_ms(),
        "error": if has_usage { Value::Null } else { json!("Sign in to Codex to see usage") },
        "status": if has_usage { "ok" } else { "error" },
    })
}

/// Scan + enrich both providers and assemble the renderer's `RateLimitState`.
async fn build_state() -> Value {
    // Both scans are filesystem walks — run them off the async runtime threads.
    let claude_snap = tokio::task::spawn_blocking(scan_claude)
        .await
        .unwrap_or_default();
    // OAuth enrichment is the only network hop; it degrades to scan-only on failure.
    let claude_snap = enrich_claude(claude_snap).await;
    let codex_snap = tokio::task::spawn_blocking(scan_codex)
        .await
        .unwrap_or_default();

    json!({
        "claude": claude_provider(&claude_snap),
        "codex": codex_provider(&codex_snap),
        // Gemini / OpenCode Go usage scanning isn't ported; leave them hidden.
        "gemini": Value::Null,
        "opencodeGo": Value::Null,
        // Desktop runs against the host runtime; WSL targeting is TUI-only.
        "claudeTarget": { "runtime": "host", "wslDistro": null },
        "codexTarget": { "runtime": "host", "wslDistro": null },
        "inactiveClaudeAccounts": [],
        "inactiveCodexAccounts": []
    })
}

#[tauri::command]
pub async fn rate_limits_get() -> Value {
    build_state().await
}

#[tauri::command]
pub async fn rate_limits_refresh() -> Value {
    build_state().await
}

// Desktop has no WSL target switching, so the per-target refreshes ignore the
// requested target and re-scan the host (matching `*Target: host` above).
#[tauri::command]
pub async fn rate_limits_refresh_codex_for_target() -> Value {
    build_state().await
}

#[tauri::command]
pub async fn rate_limits_refresh_claude_for_target() -> Value {
    build_state().await
}

#[tauri::command]
pub fn rate_limits_set_polling_interval() {}

#[tauri::command]
pub fn rate_limits_fetch_inactive_claude_accounts() {}

#[tauri::command]
pub fn rate_limits_fetch_inactive_codex_accounts() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_not_installed_maps_to_null() {
        let snap = ClaudeUsageSnapshot {
            claude_installed: false,
            ..Default::default()
        };
        assert_eq!(claude_provider(&snap), Value::Null);
    }

    #[test]
    fn claude_with_oauth_is_ok_with_windows() {
        let snap = ClaudeUsageSnapshot {
            claude_installed: true,
            five_hour_pct: Some(82.0),
            seven_day_pct: Some(40.0),
            resets_at_ms: Some(1_776_718_918_000),
            ..Default::default()
        };
        let v = claude_provider(&snap);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["error"], Value::Null);
        assert_eq!(v["session"]["usedPercent"], 82.0);
        assert_eq!(v["session"]["windowMinutes"], SESSION_WINDOW_MINUTES);
        // 5h drives the binding reset (82 >= 40), so it owns resets_at; 7d is null.
        assert_eq!(v["session"]["resetsAt"], 1_776_718_918_000i64);
        assert_eq!(v["weekly"]["usedPercent"], 40.0);
        assert_eq!(v["weekly"]["resetsAt"], Value::Null);
    }

    #[test]
    fn claude_installed_without_oauth_is_error() {
        // Installed but the OAuth fetch yielded no percentages: show "sign in",
        // never a fabricated number.
        let snap = ClaudeUsageSnapshot {
            claude_installed: true,
            ..Default::default()
        };
        let v = claude_provider(&snap);
        assert_eq!(v["status"], "error");
        assert_eq!(v["session"], Value::Null);
        assert_eq!(v["weekly"], Value::Null);
    }

    #[test]
    fn codex_window_converts_seconds_to_millis() {
        let snap = CodexUsageSnapshot {
            codex_installed: true,
            primary: Some(CodexUsageWindow {
                used_percent: 42.5,
                window_minutes: 300,
                resets_at: 1_776_718_918, // unix seconds
            }),
            ..Default::default()
        };
        let v = codex_provider(&snap);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["session"]["usedPercent"], 42.5);
        assert_eq!(v["session"]["resetsAt"], 1_776_718_918_000i64);
    }

    #[test]
    fn codex_not_installed_maps_to_null() {
        let snap = CodexUsageSnapshot {
            codex_installed: false,
            ..Default::default()
        };
        assert_eq!(codex_provider(&snap), Value::Null);
    }
}
