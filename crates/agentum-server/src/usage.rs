//! Read-only scanners for Claude Code and Codex transcript files.
//!
//! Both agents persist their session history as JSONL files in the user's
//! home directory. We tail them to surface a "plan usage %" chip in the
//! dashboard sidebar without asking the user to install any extra
//! tooling.
//!
//! Codex is the easy case: its `event_msg` records carry a
//! `rate_limits.primary.used_percent` field straight from the OpenAI API
//! response. We just need the most recent value across all session
//! files.
//!
//! Claude has no such field. Anthropic doesn't surface plan headroom in
//! the transcript, so we approximate by summing `message.usage.*`
//! tokens inside a rolling 5-hour window and exposing the raw number.
//! The dashboard maps that to a percent against the user's configured
//! plan tier (or just renders the absolute token count when no tier is
//! set).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Tokens within this window count toward the rolling-window total.
/// Claude Code's plan caps reset every 5 hours from the first request
/// in the window, so the rolling sum is the closest we can get without
/// reverse-engineering Anthropic's account API.
const CLAUDE_WINDOW: Duration = Duration::from_secs(5 * 3600);

/// Files older than this are skipped during the Claude scan. JSONL
/// transcripts from prior weeks can't contribute to a 5h window, and
/// scanning them takes a noticeable amount of wall time on busy users.
const CLAUDE_FRESHNESS_CUTOFF: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Serialize, Default)]
pub struct ClaudeUsageSnapshot {
    /// Sum of input + output + cache_creation tokens within the 5h
    /// window. `cache_read` is excluded — Anthropic doesn't bill it
    /// against the plan cap.
    pub window_tokens: u64,
    /// Unix-ms timestamp of the earliest message in the active window.
    /// `None` when no messages were found inside `CLAUDE_WINDOW`.
    pub window_start_ms: Option<i64>,
    /// `window_start_ms + 5h`. The dashboard renders a "resets in …"
    /// label from this.
    pub window_end_ms: Option<i64>,
    /// All-time token total across every JSONL file we could parse.
    /// Useful for the detail popover; not used for the % chip.
    pub all_time_tokens: u64,
    /// Per-model breakdown within the window. Keyed by the
    /// `message.model` string (e.g. `claude-opus-4-8`). Models the
    /// user hasn't touched in 5h are omitted.
    pub by_model: std::collections::BTreeMap<String, u64>,
    /// `true` when `~/.claude/projects` exists. False means the user
    /// has never run Claude Code on this host; the UI hides the chip
    /// in that case.
    pub claude_installed: bool,

    // ---- spec 001: real plan-limit + estimated cost ------------------
    // All fields below are additive `Option`s populated by the
    // `/api/usage/claude` route after merging the OAuth usage fetch.
    // They MUST stay optional so older daemons (which never set them)
    // and older clients (which never read them) both round-trip JSON
    // without breaking — see the `Helper` Deserialize below.
    /// Headline plan-limit utilization, `max(five_hour, seven_day)`,
    /// clamped 0..=100. `None` when no OAuth token was available or the
    /// fetch failed — the UI must show "unavailable", never a guess.
    pub limit_pct: Option<f64>,
    /// 5-hour rolling-window utilization (0..=100) from `/api/oauth/usage`.
    pub five_hour_pct: Option<f64>,
    /// 7-day rolling-window utilization (0..=100) from `/api/oauth/usage`.
    pub seven_day_pct: Option<f64>,
    /// Unix-ms reset time of whichever window drove `limit_pct`. Lets the
    /// UI render a "resets in …" hint without a second roundtrip.
    pub resets_at_ms: Option<i64>,
    /// Estimated USD spend for the tokens in `by_model` (the 5h window),
    /// priced with the static table in `pricing`. ESTIMATE only — there
    /// is no billing API for subscription plans. `None` when no priced
    /// model contributed.
    pub est_cost_usd: Option<f64>,
    /// Which path produced this snapshot: `"oauth"` when the plan-limit
    /// fields came from `/api/oauth/usage`, `"scan"` when we only have
    /// the local transcript scan (graceful degradation — no real %).
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CodexUsageWindow {
    pub used_percent: f64,
    pub window_minutes: u32,
    /// Unix seconds of the next reset, straight from the OpenAI
    /// response. `0` when the upstream field was absent (rare).
    pub resets_at: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CodexUsageSnapshot {
    /// Primary (~5h) rate-limit bucket. The headline number on the
    /// chip.
    pub primary: Option<CodexUsageWindow>,
    /// Weekly bucket. Shown in the detail popover.
    pub secondary: Option<CodexUsageWindow>,
    /// `"plus"`, `"pro"`, `"enterprise"`, etc. Forwarded verbatim from
    /// the OpenAI response; the UI uses it as a label.
    pub plan_type: Option<String>,
    /// `true` when `~/.codex/sessions` exists.
    pub codex_installed: bool,
}

fn walk_jsonl(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        }
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Hand-rolled ISO-8601 parser. Format is always
// `YYYY-MM-DDTHH:MM:SS[.fff]Z` in both Claude and Codex JSONL — pulling
// in chrono just for this would bloat the binary for negligible value.
pub(crate) fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: i64 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day: i64 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    let hour: i64 = std::str::from_utf8(&bytes[11..13]).ok()?.parse().ok()?;
    let min: i64 = std::str::from_utf8(&bytes[14..16]).ok()?.parse().ok()?;
    let sec: i64 = std::str::from_utf8(&bytes[17..19]).ok()?.parse().ok()?;
    let mut frac_ms: i64 = 0;
    if bytes.len() > 20 && bytes[19] == b'.' {
        let mut i = 20;
        let mut digits = 0;
        while i < bytes.len() && digits < 3 && bytes[i].is_ascii_digit() {
            frac_ms = frac_ms * 10 + (bytes[i] - b'0') as i64;
            i += 1;
            digits += 1;
        }
        for _ in digits..3 {
            frac_ms *= 10;
        }
    }
    // Days-from-civil (Howard Hinnant). Treats input as UTC.
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month;
    let d = day;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(secs * 1000 + frac_ms)
}

/// Build a snapshot of Claude usage from the local transcript store.
///
/// Pure I/O against `~/.claude/projects`; no network, no DB. Returns
/// an empty snapshot (with `claude_installed=false`) when the
/// directory doesn't exist.
pub fn scan_claude() -> ClaudeUsageSnapshot {
    let mut snap = ClaudeUsageSnapshot::default();
    let home = match home_dir() {
        Some(h) => h,
        None => return snap,
    };
    let projects = home.join(".claude").join("projects");
    let transcripts = home.join(".claude").join("transcripts");
    snap.claude_installed = projects.exists() || transcripts.exists();
    if !snap.claude_installed {
        return snap;
    }

    let now = now_ms();
    let window_floor_ms = now - CLAUDE_WINDOW.as_millis() as i64;
    let freshness_floor = SystemTime::now() - CLAUDE_FRESHNESS_CUTOFF;

    let mut files: Vec<PathBuf> = Vec::new();
    for root in [&projects, &transcripts] {
        if root.exists() {
            files.extend(walk_jsonl(root));
        }
    }

    let mut earliest_in_window: Option<i64> = None;

    for path in files {
        // Skip files whose mtime is older than the freshness cutoff —
        // they can't contribute to the 5h window and their size is the
        // dominant cost of the scan.
        let want_window = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(mtime) => mtime >= freshness_floor,
            Err(_) => false,
        };

        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            // Cheap pre-filter: real usage lines contain `"usage"`.
            // Avoids the JSON parse cost for 90%+ of lines.
            if !line.contains("\"usage\"") {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let msg = match v.get("message") {
                Some(m) => m,
                None => continue,
            };
            let usage = match msg.get("usage") {
                Some(u) => u,
                None => continue,
            };
            let input = usage
                .get("input_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let cache_create = usage
                .get("cache_creation_input_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let billable = input + output + cache_create;
            if billable == 0 {
                continue;
            }
            snap.all_time_tokens = snap.all_time_tokens.saturating_add(billable);

            if !want_window {
                continue;
            }
            let ts_ms = match v.get("timestamp").and_then(|t| t.as_str()) {
                Some(s) => match parse_iso8601_ms(s) {
                    Some(ms) => ms,
                    None => continue,
                },
                None => continue,
            };
            if ts_ms < window_floor_ms || ts_ms > now + 60_000 {
                continue;
            }
            snap.window_tokens = snap.window_tokens.saturating_add(billable);
            earliest_in_window = Some(match earliest_in_window {
                Some(e) => e.min(ts_ms),
                None => ts_ms,
            });

            if let Some(model) = msg.get("model").and_then(|m| m.as_str()) {
                *snap.by_model.entry(model.to_string()).or_insert(0) += billable;
            }
        }
    }

    if let Some(start) = earliest_in_window {
        snap.window_start_ms = Some(start);
        snap.window_end_ms = Some(start + CLAUDE_WINDOW.as_millis() as i64);
    }

    snap
}

/// Build a snapshot of Codex usage from `~/.codex/sessions`.
///
/// Walks session files newest-first and stops as soon as it finds a
/// `token_count` event — older files can only carry older snapshots.
pub fn scan_codex() -> CodexUsageSnapshot {
    let mut snap = CodexUsageSnapshot::default();
    let home = match home_dir() {
        Some(h) => h,
        None => return snap,
    };
    let sessions_dir = home.join(".codex").join("sessions");
    snap.codex_installed = sessions_dir.exists();
    if !snap.codex_installed {
        return snap;
    }

    let mut latest_ts_ms: i64 = 0;
    let mut latest_record: Option<serde_json::Value> = None;

    // Files under YYYY/MM/DD with ISO-prefixed names: lexicographic
    // sort == chronological, so reverse-walk hits newest first.
    let mut files = walk_jsonl(&sessions_dir);
    files.sort();
    for path in files.into_iter().rev() {
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        let mut found_in_file = false;
        for line in reader.lines().map_while(Result::ok) {
            if !line.contains("token_count") {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let payload = match v
                .get("payload")
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("token_count"))
            {
                Some(p) => p,
                None => continue,
            };
            if payload.get("rate_limits").is_none() {
                continue;
            }
            let ts_ms = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_iso8601_ms)
                .unwrap_or(0);
            if ts_ms >= latest_ts_ms {
                latest_ts_ms = ts_ms;
                latest_record = Some(payload.clone());
                found_in_file = true;
            }
        }
        if found_in_file {
            break;
        }
    }

    if let Some(payload) = latest_record {
        let rl = match payload.get("rate_limits") {
            Some(r) => r,
            None => return snap,
        };
        snap.plan_type = rl
            .get("plan_type")
            .and_then(|p| p.as_str())
            .map(|s| s.to_string());
        snap.primary = rl.get("primary").and_then(parse_codex_window);
        snap.secondary = rl.get("secondary").and_then(parse_codex_window);
    }

    snap
}

fn parse_codex_window(v: &serde_json::Value) -> Option<CodexUsageWindow> {
    let used = v.get("used_percent").and_then(|p| p.as_f64())?;
    let mins = v
        .get("window_minutes")
        .and_then(|m| m.as_u64())
        .unwrap_or(0) as u32;
    let resets = v.get("resets_at").and_then(|r| r.as_i64()).unwrap_or(0);
    Some(CodexUsageWindow {
        used_percent: used,
        window_minutes: mins,
        resets_at: resets,
    })
}

// ===========================================================================
// spec 001: Claude account usage — real plan-limit % + estimated cost.
//
// Design note (deviation from architecture.md): the architecture proposed
// `usage/claude_oauth.rs` and `usage/pricing.rs` submodules. We keep these as
// in-module sections here because the surface is small (one fetch fn, one
// price table) and a sibling-module split would add file churn without
// clarifying anything. The PTY-scrape fallback (`usage/claude_cli.rs`) is
// intentionally NOT implemented — see the graceful-degradation note in
// `enrich_claude` and the TODO there.
// ===========================================================================

/// Anthropic's (undocumented) Claude Code usage endpoint. Returns plan-limit
/// utilization for the 5-hour and 7-day windows of a Max/Pro subscription.
const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// `anthropic-beta` header the Claude Code CLI sends to this endpoint.
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
/// User-Agent the CLI sends; matching it keeps us aligned with the contract.
const CLAUDE_CODE_USER_AGENT: &str = "claude-code/2.1.0";
/// Upstream call timeout. The usage chip is cosmetic — a slow Anthropic
/// shouldn't stall the whole request path.
const OAUTH_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Static per-model price table (USD per single token), sourced from
/// Anthropic's public pricing as of 2026-05-28
/// (<https://www.anthropic.com/pricing>). These are list prices in
/// dollars-per-million-tokens divided by 1_000_000:
///
/// | model              | input $/Mtok | output $/Mtok | cache-write $/Mtok |
/// | ------------------ | ------------ | ------------- | ------------------ |
/// | claude-opus-4*     | 15.00        | 75.00         | 18.75              |
/// | claude-sonnet-4*   |  3.00        | 15.00         |  3.75              |
/// | claude-3-5-haiku*  |  0.80        |  4.00         |  1.00              |
///
/// The estimate uses a blended figure: the local scan only tracks a single
/// summed token count per model (`by_model` = input + output + cache_create),
/// so we apply the model's *input* price as a conservative lower-bound proxy.
/// This is explicitly an ESTIMATE; the UI labels it "est.". Models not in the
/// table are skipped (we don't guess prices for unknown models).
fn model_input_price_per_token(model: &str) -> Option<f64> {
    let m = model.to_ascii_lowercase();
    // Match on family prefixes so dated point releases (…-4-8, …-4-20250514)
    // all resolve without a table entry per build.
    if m.contains("opus") {
        Some(15.00 / 1_000_000.0)
    } else if m.contains("sonnet") {
        Some(3.00 / 1_000_000.0)
    } else if m.contains("haiku") {
        Some(0.80 / 1_000_000.0)
    } else {
        None
    }
}

/// Estimate window spend from the per-model token breakdown. Sums
/// `tokens * input_price` for every model we have a price for; returns
/// `None` when no priced model contributed so the UI can fall back to an
/// em-dash rather than render a misleading `$0.00`.
pub(crate) fn estimate_cost_usd(by_model: &std::collections::BTreeMap<String, u64>) -> Option<f64> {
    let mut total = 0.0;
    let mut priced_any = false;
    for (model, &tokens) in by_model {
        if let Some(price) = model_input_price_per_token(model) {
            total += tokens as f64 * price;
            priced_any = true;
        }
    }
    priced_any.then_some(total)
}

/// Pick the headline plan-limit % from the two windows. The user cares about
/// whichever ceiling they'll hit first, so we surface the higher of the two.
/// Both inputs are clamped 0..=100; the result is too.
pub(crate) fn pick_limit_pct(five_hour: Option<f64>, seven_day: Option<f64>) -> Option<f64> {
    let clamp = |v: f64| v.clamp(0.0, 100.0);
    match (five_hour, seven_day) {
        (Some(a), Some(b)) => Some(clamp(a.max(b))),
        (Some(a), None) => Some(clamp(a)),
        (None, Some(b)) => Some(clamp(b)),
        (None, None) => None,
    }
}

/// One OAuth credential candidate read from a store, paired with its expiry so
/// the freshest can be chosen when stores diverge.
struct OAuthCred {
    token: String,
    /// `claudeAiOauth.expiresAt` (unix-ms). `None` when the store doesn't
    /// expose it — treated as "assume current" so such a token is never
    /// discarded in favour of a definitely-expired one.
    expires_at_ms: Option<i64>,
}

/// Read the Claude OAuth bearer token from the host. Returns `None` when none
/// is available (API-key-only users, or no Claude install). NEVER logs the token.
///
/// `CLAUDE_CODE_OAUTH_TOKEN` wins outright when set — the explicit override
/// Claude Code itself honours; it carries no expiry to compare.
///
/// Otherwise we read EVERY store and pick the freshest token rather than taking
/// the first by source order. Claude Code keeps the same `claudeAiOauth` blob in
/// two places that DIVERGE: `~/.claude/.credentials.json` and the macOS Keychain.
/// It rotates the access token roughly hourly, and on macOS the Keychain is the
/// source of truth — the file copy goes stale. A fixed source order surfaced the
/// EXPIRED file token (→ 401 → "Sign in to Claude to see usage") while a valid
/// token sat in the Keychain. Choosing by latest expiry is platform-agnostic
/// (Linux: file only) and durable against rotation.
pub(crate) fn read_claude_oauth_token() -> Option<String> {
    if let Ok(tok) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        let tok = tok.trim();
        if !tok.is_empty() {
            return Some(tok.to_string());
        }
    }

    let mut candidates: Vec<OAuthCred> = Vec::new();

    if let Some(home) = home_dir() {
        let cred_path = home.join(".claude").join(".credentials.json");
        if let Ok(raw) = std::fs::read_to_string(&cred_path)
            && let Some(cred) = cred_from_credentials_json(&raw)
        {
            candidates.push(cred);
        }
    }

    // macOS Keychain. Best-effort; no-op on Linux. Claude Code stores the same
    // `.credentials.json` JSON blob under a generic-password Keychain entry.
    #[cfg(target_os = "macos")]
    {
        if let Some(cred) = read_macos_keychain_cred() {
            candidates.push(cred);
        }
    }

    pick_freshest_token(candidates, now_ms())
}

/// Choose the freshest usable token: prefer one whose expiry is still in the
/// future and, among those, the latest-expiring. A candidate with no known
/// expiry is treated as current so it's never dropped in favour of a known-
/// expired token. When every candidate is expired we still return the least-
/// expired one — the OAuth fetch will 401 and we degrade to scan, no worse than
/// before. Returns `None` only when there are no candidates at all.
fn pick_freshest_token(creds: Vec<OAuthCred>, now_ms: i64) -> Option<String> {
    creds
        .into_iter()
        .max_by_key(|c| {
            let exp = c.expires_at_ms.unwrap_or(i64::MAX);
            (exp > now_ms, exp)
        })
        .map(|c| c.token)
}

/// Extract the access token + expiry from a `.credentials.json` blob.
fn cred_from_credentials_json(raw: &str) -> Option<OAuthCred> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let oauth = v.get("claudeAiOauth")?;
    let tok = oauth.get("accessToken")?.as_str()?.trim();
    if tok.is_empty() {
        return None;
    }
    Some(OAuthCred {
        token: tok.to_string(),
        expires_at_ms: oauth.get("expiresAt").and_then(|e| e.as_i64()),
    })
}

/// Thin token-only wrapper retained for the existing extraction test.
#[cfg(test)]
fn token_from_credentials_json(raw: &str) -> Option<String> {
    cred_from_credentials_json(raw).map(|c| c.token)
}

#[cfg(target_os = "macos")]
fn read_macos_keychain_cred() -> Option<OAuthCred> {
    // Claude Code 2.1 stores credentials under the generic-password service
    // "Claude Code-credentials". `security find-generic-password -w` prints
    // the raw secret (the same JSON shape as the file) to stdout. We never
    // log the output. Failure (no entry, locked keychain) → None.
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    cred_from_credentials_json(raw.trim())
}

/// Parsed `/api/oauth/usage` result. All fields optional — Anthropic may omit
/// a window for an account that doesn't have it.
#[derive(Debug, Default, Clone)]
pub(crate) struct OAuthUsage {
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
    /// Reset time of whichever window is the binding constraint
    /// (`max(five_hour, seven_day)`), as unix-ms.
    pub resets_at_ms: Option<i64>,
}

/// Map one window object (`{ utilization, resets_at }`) to `(pct, resets_ms)`.
fn parse_oauth_window(v: Option<&serde_json::Value>) -> (Option<f64>, Option<i64>) {
    let Some(v) = v else {
        return (None, None);
    };
    let pct = v
        .get("utilization")
        .and_then(|u| u.as_f64())
        .map(|u| u.clamp(0.0, 100.0));
    let resets = v
        .get("resets_at")
        .and_then(|r| r.as_str())
        .and_then(parse_iso8601_ms);
    (pct, resets)
}

/// Fetch + parse `/api/oauth/usage`. Errors are redacted of the token. Returns
/// the parsed windows or an error string suitable for `tracing::debug!`.
async fn fetch_oauth_usage(token: &str) -> Result<OAuthUsage, String> {
    let client = reqwest::Client::builder()
        .timeout(OAUTH_FETCH_TIMEOUT)
        // reqwest reads HTTPS_PROXY / ALL_PROXY from the environment by
        // default, satisfying the spec's proxy requirement with no extra code.
        .build()
        .map_err(|e| format!("build http client: {e}"))?;

    let resp = client
        .get(OAUTH_USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", OAUTH_BETA_HEADER)
        .header(reqwest::header::USER_AGENT, CLAUDE_CODE_USER_AGENT)
        .send()
        .await
        // The token never appears in reqwest's Display for a GET error, but
        // be defensive: scrub anything token-shaped from the message.
        .map_err(|e| redact_token(&e.to_string(), token))?;

    if !resp.status().is_success() {
        return Err(format!("oauth usage returned {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| redact_token(&e.to_string(), token))?;

    let (five_hour_pct, five_resets) = parse_oauth_window(body.get("five_hour"));
    let (seven_day_pct, seven_resets) = parse_oauth_window(body.get("seven_day"));

    // resets_at follows whichever window is the binding constraint.
    let resets_at_ms = match (five_hour_pct, seven_day_pct) {
        (Some(a), Some(b)) if a >= b => five_resets,
        (Some(_), Some(_)) => seven_resets,
        (Some(_), None) => five_resets,
        (None, Some(_)) => seven_resets,
        (None, None) => None,
    };

    Ok(OAuthUsage {
        five_hour_pct,
        seven_day_pct,
        resets_at_ms,
    })
}

/// Redact a bearer token from a string before it can be logged or returned.
fn redact_token(msg: &str, token: &str) -> String {
    if token.is_empty() {
        return msg.to_string();
    }
    msg.replace(token, "<redacted>")
}

/// Take a freshly-scanned [`ClaudeUsageSnapshot`] and enrich it with the
/// real plan-limit % (from `/api/oauth/usage`) and an estimated cost. This is
/// the async half that the route runs after `spawn_blocking(scan_claude)`.
///
/// Sets `source = "oauth"` when the OAuth fetch succeeds, `source = "scan"`
/// otherwise (no token, or fetch failed). On the `scan` path `limit_pct`
/// stays `None` so the UI shows "unavailable" rather than a wrong number —
/// graceful degradation in place of the spec's PTY scrape.
//
// TODO(spec-001): optional `claude`-CLI plan-usage scrape as a richer
// fallback. Deferred deliberately — a robust server-side PTY scrape of
// Claude's interactive palette is high-risk and large; graceful degradation
// (source="scan", no band) is the safer minimum.
pub async fn enrich_claude(mut snap: ClaudeUsageSnapshot) -> ClaudeUsageSnapshot {
    // Estimated cost is local-only; compute it regardless of OAuth success.
    snap.est_cost_usd = estimate_cost_usd(&snap.by_model);

    let Some(token) = read_claude_oauth_token() else {
        snap.source = Some("scan".to_string());
        return snap;
    };

    match fetch_oauth_usage(&token).await {
        Ok(usage) => {
            snap.five_hour_pct = usage.five_hour_pct;
            snap.seven_day_pct = usage.seven_day_pct;
            snap.limit_pct = pick_limit_pct(usage.five_hour_pct, usage.seven_day_pct);
            snap.resets_at_ms = usage.resets_at_ms;
            snap.source = Some("oauth".to_string());
        }
        Err(e) => {
            // Never log the token; `e` is already redacted.
            tracing::debug!(error = %e, "claude oauth usage fetch failed; degrading to scan");
            snap.source = Some("scan".to_string());
        }
    }

    snap
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageBundle {
    pub claude: ClaudeUsageSnapshot,
    pub codex: CodexUsageSnapshot,
    /// Unix-ms wall clock at the time of the scan. The dashboard uses
    /// this for the "as of …" tooltip and to drive the
    /// "resets in N min" countdown without needing a separate
    /// roundtrip.
    pub generated_at_ms: i64,
}

pub fn scan_all() -> UsageBundle {
    UsageBundle {
        claude: scan_claude(),
        codex: scan_codex(),
        generated_at_ms: now_ms(),
    }
}

// Manual Deserialize impls so the snapshots round-trip cleanly in
// tests that replay cached payloads.
impl<'de> Deserialize<'de> for ClaudeUsageSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(default)]
            window_tokens: u64,
            #[serde(default)]
            window_start_ms: Option<i64>,
            #[serde(default)]
            window_end_ms: Option<i64>,
            #[serde(default)]
            all_time_tokens: u64,
            #[serde(default)]
            by_model: std::collections::BTreeMap<String, u64>,
            #[serde(default)]
            claude_installed: bool,
            // spec 001 additive fields — `default` so payloads predating
            // them (older daemons, replayed cache fixtures) still parse.
            #[serde(default)]
            limit_pct: Option<f64>,
            #[serde(default)]
            five_hour_pct: Option<f64>,
            #[serde(default)]
            seven_day_pct: Option<f64>,
            #[serde(default)]
            resets_at_ms: Option<i64>,
            #[serde(default)]
            est_cost_usd: Option<f64>,
            #[serde(default)]
            source: Option<String>,
        }
        let h = Helper::deserialize(deserializer)?;
        Ok(ClaudeUsageSnapshot {
            window_tokens: h.window_tokens,
            window_start_ms: h.window_start_ms,
            window_end_ms: h.window_end_ms,
            all_time_tokens: h.all_time_tokens,
            by_model: h.by_model,
            claude_installed: h.claude_installed,
            limit_pct: h.limit_pct,
            five_hour_pct: h.five_hour_pct,
            seven_day_pct: h.seven_day_pct,
            resets_at_ms: h.resets_at_ms,
            est_cost_usd: h.est_cost_usd,
            source: h.source,
        })
    }
}

impl<'de> Deserialize<'de> for CodexUsageSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(default)]
            primary: Option<CodexUsageWindow>,
            #[serde(default)]
            secondary: Option<CodexUsageWindow>,
            #[serde(default)]
            plan_type: Option<String>,
            #[serde(default)]
            codex_installed: bool,
        }
        let h = Helper::deserialize(deserializer)?;
        Ok(CodexUsageSnapshot {
            primary: h.primary,
            secondary: h.secondary,
            plan_type: h.plan_type,
            codex_installed: h.codex_installed,
        })
    }
}

impl<'de> Deserialize<'de> for CodexUsageWindow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(default)]
            used_percent: f64,
            #[serde(default)]
            window_minutes: u32,
            #[serde(default)]
            resets_at: i64,
        }
        let h = Helper::deserialize(deserializer)?;
        Ok(CodexUsageWindow {
            used_percent: h.used_percent,
            window_minutes: h.window_minutes,
            resets_at: h.resets_at,
        })
    }
}

// ===========================================================================
// Stats aggregation (Mission Control). Separate from the 5h-window chip above:
// the chip lumps `billable = input+output+cache_create` and drops cache_read;
// the stats surface needs all four token classes kept apart, plus project /
// session / model attribution. Pure parsers + pure aggregators (testable
// without the filesystem) sit under thin path-resolving wrappers.
// ===========================================================================

/// One Claude usage-bearing assistant record, fully attributed.
#[derive(Clone)]
pub(crate) struct ParsedClaudeRecord {
    pub ts_ms: i64,
    pub day: String,
    #[allow(dead_code)] // captured for attribution; not yet surfaced in the dashboard
    pub project: String,
    pub project_label: String,
    pub branch: Option<String>,
    pub session_id: String,
    pub model: Option<String>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// Day key = the UTC calendar day, sliced straight off the ISO-8601 prefix
/// (`YYYY-MM-DD`). Cheaper and timezone-stable vs. recomputing from epoch ms.
fn iso_day(ts: &str) -> Option<String> {
    if ts.len() >= 10 && ts.as_bytes()[4] == b'-' && ts.as_bytes()[7] == b'-' {
        Some(ts[0..10].to_string())
    } else {
        None
    }
}

/// Human label for a project path = its final path segment.
fn project_label_from_path(cwd: &str) -> String {
    cwd.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(cwd)
        .to_string()
}

pub(crate) fn parse_claude_usage_record(line: &str) -> Option<ParsedClaudeRecord> {
    if !line.contains("\"usage\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let msg = v.get("message")?;
    let usage = msg.get("usage")?;
    let g = |k: &str| usage.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let input = g("input_tokens");
    let output = g("output_tokens");
    let cache_write = g("cache_creation_input_tokens");
    let cache_read = g("cache_read_input_tokens");
    if input + output + cache_write + cache_read == 0 {
        return None;
    }
    let ts = v.get("timestamp").and_then(|t| t.as_str())?;
    let ts_ms = parse_iso8601_ms(ts)?;
    let day = iso_day(ts)?;
    let cwd = v
        .get("cwd")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    Some(ParsedClaudeRecord {
        ts_ms,
        day,
        project_label: project_label_from_path(&cwd),
        project: cwd,
        branch: v
            .get("gitBranch")
            .and_then(|b| b.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        session_id: v
            .get("sessionId")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        model: msg.get("model").and_then(|m| m.as_str()).map(String::from),
        input,
        output,
        cache_read,
        cache_write,
    })
}

/// One Codex `token_count` record's per-turn delta (`last_token_usage`).
#[derive(Clone)]
pub(crate) struct ParsedCodexRecord {
    pub ts_ms: i64,
    pub day: String,
    pub session_id: String,
    pub model: Option<String>,
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
    pub reasoning_output: u64,
    pub total: u64,
}

pub(crate) fn parse_codex_usage_record(
    line: &str,
    session_id: &str,
    model: Option<&str>,
) -> Option<ParsedCodexRecord> {
    if !line.contains("token_count") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload = v.get("payload")?;
    if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
        return None;
    }
    let last = payload.get("info")?.get("last_token_usage")?;
    let g = |k: &str| last.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let input = g("input_tokens");
    let cached_input = g("cached_input_tokens");
    let output = g("output_tokens");
    let reasoning_output = g("reasoning_output_tokens");
    let total = g("total_tokens");
    if total == 0 && input + output == 0 {
        return None;
    }
    let ts = v.get("timestamp").and_then(|t| t.as_str())?;
    Some(ParsedCodexRecord {
        ts_ms: parse_iso8601_ms(ts)?,
        day: iso_day(ts)?,
        session_id: session_id.to_string(),
        model: model.map(String::from),
        input,
        cached_input,
        output,
        reasoning_output,
        total,
    })
}

/// Reporting window the UI requests.
pub enum UsageRange {
    D7,
    D30,
    D90,
    All,
}

impl UsageRange {
    pub(crate) fn from_str(s: &str) -> UsageRange {
        match s {
            "7d" => UsageRange::D7,
            "30d" => UsageRange::D30,
            "90d" => UsageRange::D90,
            _ => UsageRange::All,
        }
    }

    /// Inclusive lower bound in epoch-ms, or `None` for `All`.
    pub fn floor_ms(&self, now_ms: i64) -> Option<i64> {
        let days = match self {
            UsageRange::D7 => 7,
            UsageRange::D30 => 30,
            UsageRange::D90 => 90,
            UsageRange::All => return None,
        };
        Some(now_ms - days * 86_400_000)
    }
}

// ===========================================================================
// Task 2: Claude stats aggregation — contracts + pure cores + wrappers
// ===========================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUsageSummary {
    pub scope: String,
    pub range: String,
    pub sessions: u64,
    pub turns: u64,
    pub zero_cache_read_turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_reuse_rate: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub top_model: Option<String>,
    pub top_project: Option<String>,
    pub has_any_claude_data: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUsageDailyPoint {
    pub day: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUsageBreakdownRow {
    pub key: String,
    pub label: String,
    pub sessions: u64,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUsageSessionRow {
    pub session_id: String,
    pub last_active_at: String,
    pub duration_minutes: u64,
    pub project_label: String,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

// Per-Mtok USD rates by model family. cache_read ≈ 0.1× input (Anthropic
// standard). ESTIMATE only — unknown model ⇒ None (no contribution).
struct ClaudeRates {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

fn claude_rates(model: &str) -> Option<ClaudeRates> {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        Some(ClaudeRates {
            input: 15.0,
            output: 75.0,
            cache_write: 18.75,
            cache_read: 1.50,
        })
    } else if m.contains("sonnet") {
        Some(ClaudeRates {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.30,
        })
    } else if m.contains("haiku") {
        Some(ClaudeRates {
            input: 0.80,
            output: 4.0,
            cache_write: 1.0,
            cache_read: 0.08,
        })
    } else {
        None
    }
}

fn claude_cost(model: Option<&str>, input: u64, output: u64, cw: u64, cr: u64) -> Option<f64> {
    let r = claude_rates(model?)?;
    let m = 1_000_000.0_f64;
    Some(
        input as f64 * r.input / m
            + output as f64 * r.output / m
            + cw as f64 * r.cache_write / m
            + cr as f64 * r.cache_read / m,
    )
}

fn claude_in_range(
    records: Vec<ParsedClaudeRecord>,
    range: UsageRange,
    now_ms: i64,
) -> Vec<ParsedClaudeRecord> {
    match range.floor_ms(now_ms) {
        Some(floor) => records.into_iter().filter(|r| r.ts_ms >= floor).collect(),
        None => records,
    }
}

fn range_label(r: &UsageRange) -> String {
    match r {
        UsageRange::D7 => "7d",
        UsageRange::D30 => "30d",
        UsageRange::D90 => "90d",
        UsageRange::All => "all",
    }
    .to_string()
}

// ---- pure aggregation cores (no filesystem, fully testable) ---------------

pub(crate) fn claude_usage_summary_from_records(
    records: Vec<ParsedClaudeRecord>,
    scope: &str,
    range: UsageRange,
    now_ms: i64,
) -> ClaudeUsageSummary {
    let range_str = range_label(&range);
    let records = claude_in_range(records, range, now_ms);
    let mut sessions = std::collections::BTreeSet::new();
    let (mut input, mut output, mut cr, mut cw, mut zero_cr) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut cost = 0.0_f64;
    let mut cost_any = false;
    let mut by_model: std::collections::BTreeMap<String, u64> = Default::default();
    let mut by_project: std::collections::BTreeMap<String, u64> = Default::default();
    for r in &records {
        sessions.insert(r.session_id.clone());
        input += r.input;
        output += r.output;
        cr += r.cache_read;
        cw += r.cache_write;
        if r.cache_read == 0 {
            zero_cr += 1;
        }
        if let Some(c) = claude_cost(
            r.model.as_deref(),
            r.input,
            r.output,
            r.cache_write,
            r.cache_read,
        ) {
            cost += c;
            cost_any = true;
        }
        let tot = r.input + r.output + r.cache_read + r.cache_write;
        if let Some(m) = &r.model {
            *by_model.entry(m.clone()).or_default() += tot;
        }
        *by_project.entry(r.project_label.clone()).or_default() += tot;
    }
    let top = |m: &std::collections::BTreeMap<String, u64>| -> Option<String> {
        m.iter().max_by_key(|(_, v)| **v).map(|(k, _)| k.clone())
    };
    let denom = cr + input;
    ClaudeUsageSummary {
        scope: scope.to_string(),
        range: range_str,
        sessions: sessions.len() as u64,
        turns: records.len() as u64,
        zero_cache_read_turns: zero_cr,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cr,
        cache_write_tokens: cw,
        cache_reuse_rate: if denom > 0 {
            Some(cr as f64 / denom as f64)
        } else {
            None
        },
        estimated_cost_usd: cost_any.then_some(cost),
        top_model: top(&by_model),
        top_project: top(&by_project),
        has_any_claude_data: !records.is_empty(),
    }
}

pub(crate) fn claude_usage_daily_from_records(
    records: Vec<ParsedClaudeRecord>,
) -> Vec<ClaudeUsageDailyPoint> {
    let mut by_day: std::collections::BTreeMap<String, ClaudeUsageDailyPoint> = Default::default();
    for r in records {
        let e = by_day
            .entry(r.day.clone())
            .or_insert(ClaudeUsageDailyPoint {
                day: r.day.clone(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            });
        e.input_tokens += r.input;
        e.output_tokens += r.output;
        e.cache_read_tokens += r.cache_read;
        e.cache_write_tokens += r.cache_write;
    }
    // BTreeMap iterates in ascending key order → ascending by day.
    by_day.into_values().collect()
}

pub(crate) fn claude_usage_breakdown_from_records(
    records: Vec<ParsedClaudeRecord>,
    kind: &str,
) -> Vec<ClaudeUsageBreakdownRow> {
    struct Acc {
        label: String,
        sessions: std::collections::BTreeSet<String>,
        turns: u64,
        input: u64,
        output: u64,
        cr: u64,
        cw: u64,
        cost: f64,
        cost_any: bool,
    }
    let mut groups: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        let (key, label) = if kind == "project" {
            (r.project_label.clone(), r.project_label.clone())
        } else {
            // Default to "model" breakdown; unknown model ⇒ "unknown".
            let m = r.model.clone().unwrap_or_else(|| "unknown".to_string());
            (m.clone(), m)
        };
        let a = groups.entry(key.clone()).or_insert(Acc {
            label,
            sessions: Default::default(),
            turns: 0,
            input: 0,
            output: 0,
            cr: 0,
            cw: 0,
            cost: 0.0,
            cost_any: false,
        });
        a.sessions.insert(r.session_id.clone());
        a.turns += 1;
        a.input += r.input;
        a.output += r.output;
        a.cr += r.cache_read;
        a.cw += r.cache_write;
        if let Some(c) = claude_cost(
            r.model.as_deref(),
            r.input,
            r.output,
            r.cache_write,
            r.cache_read,
        ) {
            a.cost += c;
            a.cost_any = true;
        }
    }
    let mut rows: Vec<ClaudeUsageBreakdownRow> = groups
        .into_iter()
        .map(|(key, a)| ClaudeUsageBreakdownRow {
            key,
            label: a.label,
            sessions: a.sessions.len() as u64,
            turns: a.turns,
            input_tokens: a.input,
            output_tokens: a.output,
            cache_read_tokens: a.cr,
            cache_write_tokens: a.cw,
            estimated_cost_usd: a.cost_any.then_some(a.cost),
        })
        .collect();
    // Highest total-token rows first.
    rows.sort_by(|x, y| {
        (y.input_tokens + y.output_tokens + y.cache_read_tokens + y.cache_write_tokens)
            .cmp(&(x.input_tokens + x.output_tokens + x.cache_read_tokens + x.cache_write_tokens))
    });
    rows
}

pub(crate) fn claude_usage_recent_sessions_from_records(
    records: Vec<ParsedClaudeRecord>,
    limit: usize,
) -> Vec<ClaudeUsageSessionRow> {
    struct Acc {
        first_ms: i64,
        last_ms: i64,
        last_day: String,
        project_label: String,
        branch: Option<String>,
        model: Option<String>,
        turns: u64,
        input: u64,
        output: u64,
        cr: u64,
        cw: u64,
    }
    let mut by_session: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        let a = by_session.entry(r.session_id.clone()).or_insert(Acc {
            first_ms: r.ts_ms,
            last_ms: r.ts_ms,
            last_day: r.day.clone(),
            project_label: r.project_label.clone(),
            branch: r.branch.clone(),
            model: r.model.clone(),
            turns: 0,
            input: 0,
            output: 0,
            cr: 0,
            cw: 0,
        });
        a.first_ms = a.first_ms.min(r.ts_ms);
        if r.ts_ms >= a.last_ms {
            a.last_ms = r.ts_ms;
            a.last_day = r.day.clone();
            // Track model from the latest record (most representative).
            a.model = r.model.clone();
        }
        a.turns += 1;
        a.input += r.input;
        a.output += r.output;
        a.cr += r.cache_read;
        a.cw += r.cache_write;
    }
    let mut rows: Vec<(i64, ClaudeUsageSessionRow)> = by_session
        .into_iter()
        .map(|(session_id, a)| {
            (
                a.last_ms,
                ClaudeUsageSessionRow {
                    session_id,
                    // The record dropped the raw ISO string; the day string
                    // (YYYY-MM-DD) is sufficient precision for the UI's
                    // "last active" label.
                    last_active_at: a.last_day,
                    duration_minutes: ((a.last_ms - a.first_ms).max(0) / 60_000) as u64,
                    project_label: a.project_label,
                    branch: a.branch,
                    model: a.model,
                    turns: a.turns,
                    input_tokens: a.input,
                    output_tokens: a.output,
                    cache_read_tokens: a.cr,
                    cache_write_tokens: a.cw,
                },
            )
        })
        .collect();
    // Most-recently-active sessions first.
    rows.sort_by(|x, y| y.0.cmp(&x.0));
    rows.into_iter().take(limit).map(|(_, row)| row).collect()
}

// ---- path-resolving public wrappers (desktop commands call these) ----------

fn claude_log_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = home_dir() {
        for sub in [".claude/projects", ".claude/transcripts"] {
            let root = home.join(sub);
            if root.exists() {
                files.extend(walk_jsonl(&root));
            }
        }
    }
    files
}

// ---------------------------------------------------------------------------
// Parsed-record cache
//
// The scan+parse of ~/.claude and ~/.codex logs is the expensive part;
// summary/daily/breakdown/recent all derive from the same parsed records,
// so we scan once and reuse until an explicit refresh invalidates.
// Arc<Vec<…>> so a cache hit is a cheap pointer clone, not a deep Vec copy.
// ---------------------------------------------------------------------------

static CLAUDE_RECORD_CACHE: LazyLock<RwLock<Option<Arc<Vec<ParsedClaudeRecord>>>>> =
    LazyLock::new(|| RwLock::new(None));
static CODEX_RECORD_CACHE: LazyLock<RwLock<Option<Arc<Vec<ParsedCodexRecord>>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Invalidate both provider caches so the next call re-scans from disk.
pub fn invalidate_usage_cache() {
    *CLAUDE_RECORD_CACHE.write().unwrap() = None;
    *CODEX_RECORD_CACHE.write().unwrap() = None;
}

/// Test-only helper: reports whether the Claude record cache is currently
/// populated (i.e. a scan result is stored).
#[cfg(test)]
pub(crate) fn claude_cache_is_populated() -> bool {
    CLAUDE_RECORD_CACHE.read().unwrap().is_some()
}

fn claude_scan_records() -> Vec<ParsedClaudeRecord> {
    let mut out = Vec::new();
    for path in claude_log_files() {
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some(r) = parse_claude_usage_record(&line) {
                out.push(r);
            }
        }
    }
    out
}

fn collect_claude_records() -> Vec<ParsedClaudeRecord> {
    // Check cache under a short-lived read lock; don't hold it across the scan.
    if let Some(cached) = CLAUDE_RECORD_CACHE.read().unwrap().as_ref() {
        return (**cached).clone();
    }
    let records = Arc::new(claude_scan_records());
    *CLAUDE_RECORD_CACHE.write().unwrap() = Some(Arc::clone(&records));
    (*records).clone()
}

/// Returns true when the user has any Claude Code history on this machine.
pub fn claude_has_any_data() -> bool {
    home_dir()
        .map(|h| h.join(".claude/projects").exists() || h.join(".claude/transcripts").exists())
        .unwrap_or(false)
}

pub fn claude_usage_summary(scope: &str, range: &str) -> ClaudeUsageSummary {
    claude_usage_summary_from_records(
        collect_claude_records(),
        scope,
        UsageRange::from_str(range),
        now_ms(),
    )
}

pub fn claude_usage_daily(_scope: &str, range: &str) -> Vec<ClaudeUsageDailyPoint> {
    // TODO: scope=="agentum" treated as "all" in v1 — no path filter yet.
    claude_usage_daily_from_records(claude_in_range(
        collect_claude_records(),
        UsageRange::from_str(range),
        now_ms(),
    ))
}

pub fn claude_usage_breakdown(
    _scope: &str,
    range: &str,
    kind: &str,
) -> Vec<ClaudeUsageBreakdownRow> {
    // TODO: scope=="agentum" treated as "all" in v1 — no path filter yet.
    claude_usage_breakdown_from_records(
        claude_in_range(
            collect_claude_records(),
            UsageRange::from_str(range),
            now_ms(),
        ),
        kind,
    )
}

pub fn claude_usage_recent_sessions(
    _scope: &str,
    range: &str,
    limit: usize,
) -> Vec<ClaudeUsageSessionRow> {
    // TODO: scope=="agentum" treated as "all" in v1 — no path filter yet.
    claude_usage_recent_sessions_from_records(
        claude_in_range(
            collect_claude_records(),
            UsageRange::from_str(range),
            now_ms(),
        ),
        limit,
    )
}

// ===========================================================================
// Task 3: Codex stats aggregation — contracts + pure cores + wrappers
//
// Codex pricing is subscription-based with no public per-model billing API,
// so estimated_cost_usd is always None and has_inferred_pricing is always true.
// Model is extracted from the first `turn_context` record (payload.model);
// falls back to None if absent — breakdown groups under the model key "codex".
// ===========================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageSummary {
    pub scope: String,
    pub range: String,
    pub sessions: u64,
    pub events: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    /// Always None — no per-model Codex billing source.
    pub estimated_cost_usd: Option<f64>,
    pub top_model: Option<String>,
    pub top_project: Option<String>,
    pub has_any_codex_data: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageDailyPoint {
    pub day: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageBreakdownRow {
    pub key: String,
    pub label: String,
    pub sessions: u64,
    pub events: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    /// Always None — subscription pricing can't be decomposed per-call.
    pub estimated_cost_usd: Option<f64>,
    pub has_inferred_pricing: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageSessionRow {
    pub session_id: String,
    pub last_active_at: String,
    pub duration_minutes: u64,
    pub project_label: String,
    pub model: Option<String>,
    pub events: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub has_inferred_pricing: bool,
}

fn codex_in_range(
    records: Vec<ParsedCodexRecord>,
    range: UsageRange,
    now_ms: i64,
) -> Vec<ParsedCodexRecord> {
    match range.floor_ms(now_ms) {
        Some(floor) => records.into_iter().filter(|r| r.ts_ms >= floor).collect(),
        None => records,
    }
}

pub(crate) fn codex_usage_summary_from_records(
    records: Vec<ParsedCodexRecord>,
    scope: &str,
    range: UsageRange,
    now_ms: i64,
) -> CodexUsageSummary {
    let range_str = range_label(&range);
    let records = codex_in_range(records, range, now_ms);
    let mut sessions = std::collections::BTreeSet::new();
    let (mut i, mut ci, mut o, mut ro, mut tot) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut by_model: std::collections::BTreeMap<String, u64> = Default::default();
    for r in &records {
        sessions.insert(r.session_id.clone());
        i += r.input;
        ci += r.cached_input;
        o += r.output;
        ro += r.reasoning_output;
        tot += r.total;
        if let Some(m) = &r.model {
            *by_model.entry(m.clone()).or_default() += r.total;
        }
    }
    CodexUsageSummary {
        scope: scope.to_string(),
        range: range_str,
        sessions: sessions.len() as u64,
        events: records.len() as u64,
        input_tokens: i,
        cached_input_tokens: ci,
        output_tokens: o,
        reasoning_output_tokens: ro,
        total_tokens: tot,
        estimated_cost_usd: None,
        top_model: by_model
            .iter()
            .max_by_key(|(_, v)| **v)
            .map(|(k, _)| k.clone()),
        top_project: None, // Codex JSONL carries no per-record project path in v1.
        has_any_codex_data: !records.is_empty(),
    }
}

pub(crate) fn codex_usage_daily_from_records(
    records: Vec<ParsedCodexRecord>,
) -> Vec<CodexUsageDailyPoint> {
    let mut by_day: std::collections::BTreeMap<String, CodexUsageDailyPoint> = Default::default();
    for r in records {
        let e = by_day.entry(r.day.clone()).or_insert(CodexUsageDailyPoint {
            day: r.day.clone(),
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens: 0,
        });
        e.input_tokens += r.input;
        e.cached_input_tokens += r.cached_input;
        e.output_tokens += r.output;
        e.reasoning_output_tokens += r.reasoning_output;
        e.total_tokens += r.total;
    }
    // BTreeMap iterates ascending → ascending by day.
    by_day.into_values().collect()
}

pub(crate) fn codex_usage_breakdown_from_records(
    records: Vec<ParsedCodexRecord>,
    kind: &str,
) -> Vec<CodexUsageBreakdownRow> {
    struct Acc {
        label: String,
        sessions: std::collections::BTreeSet<String>,
        events: u64,
        i: u64,
        ci: u64,
        o: u64,
        ro: u64,
        tot: u64,
    }
    let mut groups: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        // Codex JSONL has no per-record project — project breakdown gets a single
        // "codex" bucket. Model breakdown keys by model name or "codex" when absent.
        let (key, label) = if kind == "project" {
            ("codex".to_string(), "codex".to_string())
        } else {
            let m = r.model.clone().unwrap_or_else(|| "codex".to_string());
            (m.clone(), m)
        };
        let a = groups.entry(key).or_insert(Acc {
            label,
            sessions: Default::default(),
            events: 0,
            i: 0,
            ci: 0,
            o: 0,
            ro: 0,
            tot: 0,
        });
        a.sessions.insert(r.session_id.clone());
        a.events += 1;
        a.i += r.input;
        a.ci += r.cached_input;
        a.o += r.output;
        a.ro += r.reasoning_output;
        a.tot += r.total;
    }
    let mut rows: Vec<CodexUsageBreakdownRow> = groups
        .into_iter()
        .map(|(key, a)| CodexUsageBreakdownRow {
            key,
            label: a.label,
            sessions: a.sessions.len() as u64,
            events: a.events,
            input_tokens: a.i,
            cached_input_tokens: a.ci,
            output_tokens: a.o,
            reasoning_output_tokens: a.ro,
            total_tokens: a.tot,
            estimated_cost_usd: None,
            has_inferred_pricing: true,
        })
        .collect();
    rows.sort_by(|x, y| y.total_tokens.cmp(&x.total_tokens));
    rows
}

pub(crate) fn codex_usage_recent_sessions_from_records(
    records: Vec<ParsedCodexRecord>,
    limit: usize,
) -> Vec<CodexUsageSessionRow> {
    struct Acc {
        first_ms: i64,
        last_ms: i64,
        last_day: String,
        model: Option<String>,
        events: u64,
        i: u64,
        ci: u64,
        o: u64,
        ro: u64,
        tot: u64,
    }
    let mut by_session: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        let a = by_session.entry(r.session_id.clone()).or_insert(Acc {
            first_ms: r.ts_ms,
            last_ms: r.ts_ms,
            last_day: r.day.clone(),
            model: r.model.clone(),
            events: 0,
            i: 0,
            ci: 0,
            o: 0,
            ro: 0,
            tot: 0,
        });
        a.first_ms = a.first_ms.min(r.ts_ms);
        if r.ts_ms >= a.last_ms {
            a.last_ms = r.ts_ms;
            a.last_day = r.day.clone();
            // Use the model from the latest record (most representative).
            a.model = r.model.clone();
        }
        a.events += 1;
        a.i += r.input;
        a.ci += r.cached_input;
        a.o += r.output;
        a.ro += r.reasoning_output;
        a.tot += r.total;
    }
    let mut rows: Vec<(i64, CodexUsageSessionRow)> = by_session
        .into_iter()
        .map(|(session_id, a)| {
            (
                a.last_ms,
                CodexUsageSessionRow {
                    session_id,
                    last_active_at: a.last_day,
                    duration_minutes: ((a.last_ms - a.first_ms).max(0) / 60_000) as u64,
                    project_label: "codex".to_string(),
                    model: a.model,
                    events: a.events,
                    input_tokens: a.i,
                    cached_input_tokens: a.ci,
                    output_tokens: a.o,
                    reasoning_output_tokens: a.ro,
                    total_tokens: a.tot,
                    has_inferred_pricing: true,
                },
            )
        })
        .collect();
    // Most-recently-active sessions first.
    rows.sort_by(|x, y| y.0.cmp(&x.0));
    rows.into_iter().take(limit).map(|(_, row)| row).collect()
}

// ---- path-resolving public wrappers (desktop commands call these) ----------

fn codex_session_files() -> Vec<PathBuf> {
    home_dir()
        .map(|h| {
            let d = h.join(".codex/sessions");
            if d.exists() {
                walk_jsonl(&d)
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default()
}

/// Extract the model from the first `turn_context` line in a Codex JSONL file.
///
/// Recon (Task 3 Step 1) confirmed: model lives at `payload.model` on lines
/// where `type == "turn_context"`. `session_meta` has only `model_provider`,
/// not the model name. Falls back to `None` — breakdown groups under "codex".
fn codex_model_for(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(m) = v
                .get("payload")
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
            {
                return Some(m.to_string());
            }
        }
    }
    None
}

fn codex_scan_records() -> Vec<ParsedCodexRecord> {
    let mut out = Vec::new();
    for path in codex_session_files() {
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let model = codex_model_for(&path);
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some(r) = parse_codex_usage_record(&line, &session_id, model.as_deref()) {
                out.push(r);
            }
        }
    }
    out
}

fn collect_codex_records() -> Vec<ParsedCodexRecord> {
    // Check cache under a short-lived read lock; don't hold it across the scan.
    if let Some(cached) = CODEX_RECORD_CACHE.read().unwrap().as_ref() {
        return (**cached).clone();
    }
    let records = Arc::new(codex_scan_records());
    *CODEX_RECORD_CACHE.write().unwrap() = Some(Arc::clone(&records));
    (*records).clone()
}

/// Returns true when the user has any Codex history on this machine.
pub fn codex_has_any_data() -> bool {
    home_dir()
        .map(|h| h.join(".codex/sessions").exists())
        .unwrap_or(false)
}

pub fn codex_usage_summary(scope: &str, range: &str) -> CodexUsageSummary {
    codex_usage_summary_from_records(
        collect_codex_records(),
        scope,
        UsageRange::from_str(range),
        now_ms(),
    )
}

pub fn codex_usage_daily(_scope: &str, range: &str) -> Vec<CodexUsageDailyPoint> {
    // TODO: scope=="agentum" treated as "all" in v1 — no path filter yet.
    codex_usage_daily_from_records(codex_in_range(
        collect_codex_records(),
        UsageRange::from_str(range),
        now_ms(),
    ))
}

pub fn codex_usage_breakdown(_scope: &str, range: &str, kind: &str) -> Vec<CodexUsageBreakdownRow> {
    // TODO: scope=="agentum" treated as "all" in v1 — no path filter yet.
    codex_usage_breakdown_from_records(
        codex_in_range(
            collect_codex_records(),
            UsageRange::from_str(range),
            now_ms(),
        ),
        kind,
    )
}

pub fn codex_usage_recent_sessions(
    _scope: &str,
    range: &str,
    limit: usize,
) -> Vec<CodexUsageSessionRow> {
    // TODO: scope=="agentum" treated as "all" in v1 — no path filter yet.
    codex_usage_recent_sessions_from_records(
        codex_in_range(
            collect_codex_records(),
            UsageRange::from_str(range),
            now_ms(),
        ),
        limit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso8601() {
        let got = parse_iso8601_ms("2026-05-27T14:20:08.051Z").unwrap();
        assert!(got > 1_700_000_000_000);
        assert!(got < 2_000_000_000_000);
        let no_frac = parse_iso8601_ms("2026-05-27T14:20:08Z").unwrap();
        assert_eq!(got - no_frac, 51);
    }

    #[test]
    fn scan_claude_returns_sane_shape() {
        let snap = scan_claude();
        assert!(snap.all_time_tokens >= snap.window_tokens);
    }

    #[test]
    fn parse_codex_window_extracts_fields() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"used_percent":42.5,"window_minutes":300,"resets_at":1776718918}"#,
        )
        .unwrap();
        let w = parse_codex_window(&v).unwrap();
        assert_eq!(w.used_percent, 42.5);
        assert_eq!(w.window_minutes, 300);
        assert_eq!(w.resets_at, 1776718918);
    }

    // ---- spec 001 -----------------------------------------------------

    #[test]
    fn limit_pct_takes_max_of_windows() {
        // Higher of the two windows wins — the binding ceiling.
        assert_eq!(pick_limit_pct(Some(42.0), Some(81.0)), Some(81.0));
        assert_eq!(pick_limit_pct(Some(90.0), Some(10.0)), Some(90.0));
        // Single window present → that one.
        assert_eq!(pick_limit_pct(Some(55.0), None), Some(55.0));
        assert_eq!(pick_limit_pct(None, Some(55.0)), Some(55.0));
        // Neither → none (UI shows "unavailable", not a guess).
        assert_eq!(pick_limit_pct(None, None), None);
    }

    #[test]
    fn limit_pct_clamps_out_of_range() {
        assert_eq!(pick_limit_pct(Some(150.0), Some(-5.0)), Some(100.0));
        assert_eq!(pick_limit_pct(Some(-10.0), None), Some(0.0));
    }

    #[test]
    fn estimate_cost_prices_known_models_and_skips_unknown() {
        let mut by_model = std::collections::BTreeMap::new();
        // 1M opus tokens at $15/Mtok (input proxy price) = $15.00.
        by_model.insert("claude-opus-4-8".to_string(), 1_000_000u64);
        // An unknown model contributes nothing (we don't guess).
        by_model.insert("some-future-model".to_string(), 5_000_000u64);
        let cost = estimate_cost_usd(&by_model).expect("priced model present");
        assert!((cost - 15.00).abs() < 1e-9, "expected ~$15.00, got {cost}");

        // No priced model → None, so the UI can render an em-dash.
        let mut only_unknown = std::collections::BTreeMap::new();
        only_unknown.insert("mystery".to_string(), 9_999u64);
        assert_eq!(estimate_cost_usd(&only_unknown), None);

        // Empty breakdown → None.
        assert_eq!(estimate_cost_usd(&std::collections::BTreeMap::new()), None);
    }

    #[test]
    fn snapshot_deserializes_with_missing_spec001_fields() {
        // A payload from a daemon predating spec 001 carries none of the
        // new fields. It MUST still deserialize, with the new fields None.
        let legacy = r#"{
            "window_tokens": 1234,
            "all_time_tokens": 5678,
            "by_model": {"claude-opus-4-8": 1234},
            "claude_installed": true
        }"#;
        let snap: ClaudeUsageSnapshot = serde_json::from_str(legacy).expect("legacy parses");
        assert_eq!(snap.window_tokens, 1234);
        assert_eq!(snap.all_time_tokens, 5678);
        assert!(snap.claude_installed);
        assert_eq!(snap.limit_pct, None);
        assert_eq!(snap.five_hour_pct, None);
        assert_eq!(snap.seven_day_pct, None);
        assert_eq!(snap.resets_at_ms, None);
        assert_eq!(snap.est_cost_usd, None);
        assert_eq!(snap.source, None);
    }

    #[test]
    fn snapshot_round_trips_with_spec001_fields() {
        let mut snap = ClaudeUsageSnapshot {
            limit_pct: Some(82.0),
            five_hour_pct: Some(82.0),
            seven_day_pct: Some(40.0),
            resets_at_ms: Some(1_900_000_000_000),
            est_cost_usd: Some(12.40),
            source: Some("oauth".to_string()),
            ..Default::default()
        };
        snap.window_tokens = 99;
        let json = serde_json::to_string(&snap).unwrap();
        let back: ClaudeUsageSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.limit_pct, Some(82.0));
        assert_eq!(back.source.as_deref(), Some("oauth"));
        assert_eq!(back.window_tokens, 99);
    }

    #[test]
    fn token_extracted_from_credentials_json() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-xyz","refreshToken":"r"}}"#;
        assert_eq!(
            token_from_credentials_json(raw).as_deref(),
            Some("sk-ant-oat-xyz")
        );
        // Missing / empty token → None.
        assert_eq!(token_from_credentials_json(r#"{"claudeAiOauth":{}}"#), None);
        assert_eq!(token_from_credentials_json("not json"), None);
    }

    #[test]
    fn cred_parses_token_and_expiry() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":1781411281013}}"#;
        let c = cred_from_credentials_json(raw).expect("parses");
        assert_eq!(c.token, "tok");
        assert_eq!(c.expires_at_ms, Some(1_781_411_281_013));
        // Token present but no expiresAt → None expiry, still usable.
        let no_exp = cred_from_credentials_json(r#"{"claudeAiOauth":{"accessToken":"t"}}"#)
            .expect("parses without expiry");
        assert_eq!(no_exp.expires_at_ms, None);
    }

    #[test]
    fn picks_valid_token_over_expired_regardless_of_order() {
        // The real macOS bug: an expired file token shadowed a valid Keychain
        // token. The freshest must win no matter the candidate order.
        let now = 1_000_000;
        let make = |t: &str, exp: i64| OAuthCred {
            token: t.into(),
            expires_at_ms: Some(exp),
        };
        assert_eq!(
            pick_freshest_token(
                vec![make("expired", now - 1), make("valid", now + 10_000)],
                now
            )
            .as_deref(),
            Some("valid")
        );
        assert_eq!(
            pick_freshest_token(
                vec![make("valid", now + 10_000), make("expired", now - 1)],
                now
            )
            .as_deref(),
            Some("valid")
        );
    }

    #[test]
    fn picks_latest_expiring_among_valid() {
        let now = 1_000_000;
        let soon = OAuthCred {
            token: "soon".into(),
            expires_at_ms: Some(now + 1_000),
        };
        let later = OAuthCred {
            token: "later".into(),
            expires_at_ms: Some(now + 9_000),
        };
        assert_eq!(
            pick_freshest_token(vec![soon, later], now).as_deref(),
            Some("later")
        );
    }

    #[test]
    fn falls_back_to_least_expired_when_all_expired() {
        // Both expired: still attempt the fetch (it 401s → degrade to scan),
        // never silently return None when tokens exist.
        let now = 1_000_000;
        let old = OAuthCred {
            token: "old".into(),
            expires_at_ms: Some(now - 9_000),
        };
        let less_old = OAuthCred {
            token: "less_old".into(),
            expires_at_ms: Some(now - 1_000),
        };
        assert_eq!(
            pick_freshest_token(vec![old, less_old], now).as_deref(),
            Some("less_old")
        );
    }

    #[test]
    fn unknown_expiry_beats_known_expired() {
        let now = 1_000_000;
        let no_exp = OAuthCred {
            token: "no_exp".into(),
            expires_at_ms: None,
        };
        let expired = OAuthCred {
            token: "expired".into(),
            expires_at_ms: Some(now - 1),
        };
        assert_eq!(
            pick_freshest_token(vec![expired, no_exp], now).as_deref(),
            Some("no_exp")
        );
    }

    #[test]
    fn no_candidates_yields_none() {
        assert_eq!(pick_freshest_token(vec![], 1), None);
    }

    #[test]
    fn redact_token_scrubs_secret() {
        let msg = "request to https://x failed with token sk-ant-oat-xyz in url";
        let scrubbed = redact_token(msg, "sk-ant-oat-xyz");
        assert!(!scrubbed.contains("sk-ant-oat-xyz"));
        assert!(scrubbed.contains("<redacted>"));
    }

    // ---- Task 1: per-record parsers + UsageRange --------------------------

    #[test]
    fn parse_claude_record_extracts_all_four_token_fields() {
        let line = r#"{"cwd":"/Users/me/Developer/projects/agentum-tui-fresh","gitBranch":"main","sessionId":"88acb90d-1c09-41f4-9a3e-0b44fbe9aae5","timestamp":"2026-06-19T15:21:41.300Z","type":"assistant","message":{"role":"assistant","model":"claude-opus-4-7","usage":{"input_tokens":6,"output_tokens":275,"cache_creation_input_tokens":11477,"cache_read_input_tokens":26242}}}"#;
        let r = parse_claude_usage_record(line).expect("parses");
        assert_eq!(r.input, 6);
        assert_eq!(r.output, 275);
        assert_eq!(r.cache_write, 11477);
        assert_eq!(r.cache_read, 26242);
        assert_eq!(r.day, "2026-06-19");
        assert_eq!(r.project, "/Users/me/Developer/projects/agentum-tui-fresh");
        assert_eq!(r.project_label, "agentum-tui-fresh");
        assert_eq!(r.branch.as_deref(), Some("main"));
        assert_eq!(r.session_id, "88acb90d-1c09-41f4-9a3e-0b44fbe9aae5");
        assert_eq!(r.model.as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn parse_claude_record_rejects_non_usage_lines() {
        assert!(
            parse_claude_usage_record(r#"{"type":"user","message":{"role":"user"}}"#).is_none()
        );
        assert!(parse_claude_usage_record("not json").is_none());
    }

    #[test]
    fn parse_codex_record_reads_last_token_usage() {
        let line = r#"{"timestamp":"2026-04-11T01:24:44.000Z","type":"response_item","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":29422,"cached_input_tokens":5504,"output_tokens":344,"reasoning_output_tokens":124,"total_tokens":29766}}}}"#;
        let r = parse_codex_usage_record(line, "sess-1", Some("gpt-5-codex")).expect("parses");
        assert_eq!(r.input, 29422);
        assert_eq!(r.cached_input, 5504);
        assert_eq!(r.output, 344);
        assert_eq!(r.reasoning_output, 124);
        assert_eq!(r.total, 29766);
        assert_eq!(r.day, "2026-04-11");
        assert_eq!(r.session_id, "sess-1");
        assert_eq!(r.model.as_deref(), Some("gpt-5-codex"));
    }

    #[test]
    fn parse_codex_record_skips_null_info() {
        let line = r#"{"timestamp":"2026-03-31T11:08:19.000Z","payload":{"type":"token_count","info":null,"rate_limits":{}}}"#;
        assert!(parse_codex_usage_record(line, "sess-1", None).is_none());
    }

    #[test]
    fn usage_range_floor() {
        let now = 10_000_000_000i64;
        assert_eq!(
            UsageRange::from_str("7d").floor_ms(now),
            Some(now - 7 * 86_400_000)
        );
        assert_eq!(UsageRange::from_str("all").floor_ms(now), None);
        assert!(matches!(UsageRange::from_str("nonsense"), UsageRange::All));
    }

    // ---- Task 2: Claude aggregation cores ---------------------------------

    fn claude_fixture() -> Vec<ParsedClaudeRecord> {
        // Two sessions, two days, two models, two projects.
        let mk = |ts: &str,
                  day: &str,
                  proj: &str,
                  label: &str,
                  sess: &str,
                  model: &str,
                  i: u64,
                  o: u64,
                  cr: u64,
                  cw: u64| ParsedClaudeRecord {
            ts_ms: parse_iso8601_ms(ts).unwrap(),
            day: day.to_string(),
            project: proj.to_string(),
            project_label: label.to_string(),
            branch: Some("main".to_string()),
            session_id: sess.to_string(),
            model: Some(model.to_string()),
            input: i,
            output: o,
            cache_read: cr,
            cache_write: cw,
        };
        vec![
            mk(
                "2026-06-18T10:00:00Z",
                "2026-06-18",
                "/p/alpha",
                "alpha",
                "s1",
                "claude-opus-4-8",
                100,
                200,
                50,
                10,
            ),
            mk(
                "2026-06-18T11:00:00Z",
                "2026-06-18",
                "/p/alpha",
                "alpha",
                "s1",
                "claude-opus-4-8",
                0,
                5,
                0,
                0,
            ),
            mk(
                "2026-06-19T09:00:00Z",
                "2026-06-19",
                "/p/beta",
                "beta",
                "s2",
                "claude-sonnet-4-6",
                1000,
                50,
                900,
                100,
            ),
        ]
    }

    #[test]
    fn claude_summary_totals_and_tops() {
        let s = claude_usage_summary_from_records(
            claude_fixture(),
            "all",
            UsageRange::All,
            1_780_000_000_000,
        );
        assert_eq!(s.sessions, 2);
        assert_eq!(s.turns, 3);
        assert_eq!(s.input_tokens, 1100);
        assert_eq!(s.output_tokens, 255);
        assert_eq!(s.cache_read_tokens, 950);
        assert_eq!(s.cache_write_tokens, 110);
        assert_eq!(s.zero_cache_read_turns, 1); // only record 2 has cache_read == 0
        assert!(s.has_any_claude_data);
        // top_project = the one with the most total tokens (beta: 2050 vs alpha: 365).
        assert_eq!(s.top_project.as_deref(), Some("beta"));
        assert!(s.estimated_cost_usd.unwrap() > 0.0);
    }

    #[test]
    fn claude_daily_buckets_by_day() {
        let d = claude_usage_daily_from_records(claude_fixture());
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].day, "2026-06-18"); // ascending
        assert_eq!(d[0].input_tokens, 100);
        assert_eq!(d[1].day, "2026-06-19");
        assert_eq!(d[1].cache_read_tokens, 900);
    }

    #[test]
    fn claude_breakdown_by_model_and_project() {
        let bm = claude_usage_breakdown_from_records(claude_fixture(), "model");
        assert_eq!(bm.len(), 2);
        assert!(
            bm.iter()
                .any(|r| r.label == "claude-opus-4-8" && r.turns == 2)
        );
        let bp = claude_usage_breakdown_from_records(claude_fixture(), "project");
        assert!(
            bp.iter()
                .any(|r| r.label == "beta" && r.input_tokens == 1000)
        );
    }

    #[test]
    fn claude_recent_sessions_sorted_desc_and_limited() {
        let rs = claude_usage_recent_sessions_from_records(claude_fixture(), 1);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].session_id, "s2"); // most-recent lastActiveAt
        assert_eq!(rs[0].project_label, "beta");
    }

    // ---- Task 3: Codex aggregation cores ----------------------------------

    fn codex_fixture() -> Vec<ParsedCodexRecord> {
        let mk = |ts: &str, day: &str, sess: &str, i: u64, ci: u64, o: u64, ro: u64, tot: u64| {
            ParsedCodexRecord {
                ts_ms: parse_iso8601_ms(ts).unwrap(),
                day: day.to_string(),
                session_id: sess.to_string(),
                model: Some("gpt-5-codex".to_string()),
                input: i,
                cached_input: ci,
                output: o,
                reasoning_output: ro,
                total: tot,
            }
        };
        vec![
            mk(
                "2026-04-11T01:00:00Z",
                "2026-04-11",
                "c1",
                100,
                20,
                30,
                5,
                155,
            ),
            mk(
                "2026-04-11T02:00:00Z",
                "2026-04-11",
                "c1",
                200,
                0,
                40,
                0,
                240,
            ),
            mk("2026-04-12T01:00:00Z", "2026-04-12", "c2", 9, 0, 1, 0, 10),
        ]
    }

    #[test]
    fn codex_summary_totals() {
        let s = codex_usage_summary_from_records(
            codex_fixture(),
            "all",
            UsageRange::All,
            1_780_000_000_000,
        );
        assert_eq!(s.sessions, 2);
        assert_eq!(s.events, 3);
        assert_eq!(s.input_tokens, 309);
        assert_eq!(s.total_tokens, 405);
        assert!(s.has_any_codex_data);
    }

    #[test]
    fn codex_daily_and_sessions() {
        let d = codex_usage_daily_from_records(codex_fixture());
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].day, "2026-04-11");
        assert_eq!(d[0].total_tokens, 395);
        let rs = codex_usage_recent_sessions_from_records(codex_fixture(), 5);
        assert_eq!(rs[0].session_id, "c2"); // newest
        assert!(rs[0].has_inferred_pricing);
    }

    // ---- cache mechanics --------------------------------------------------

    #[test]
    fn cache_populates_on_first_collect_and_clears_on_invalidate() {
        // Start clean so the test is independent of execution order.
        invalidate_usage_cache();
        assert!(
            !claude_cache_is_populated(),
            "cache must be None after invalidate"
        );

        // First call — triggers a scan (may return empty on machines
        // with no ~/.claude data, but that's fine; an empty Vec is still
        // cached as Some).
        let first = collect_claude_records();
        assert!(
            claude_cache_is_populated(),
            "cache must be Some after first collect"
        );

        // Second call — must return same length from cache, not re-scan.
        let second = collect_claude_records();
        assert_eq!(
            first.len(),
            second.len(),
            "cache hit must return equal-length result"
        );

        // Invalidate wipes the cache.
        invalidate_usage_cache();
        assert!(
            !claude_cache_is_populated(),
            "cache must be None after second invalidate"
        );
    }
}
