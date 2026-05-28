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
fn parse_iso8601_ms(s: &str) -> Option<i64> {
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
        }
        let h = Helper::deserialize(deserializer)?;
        Ok(ClaudeUsageSnapshot {
            window_tokens: h.window_tokens,
            window_start_ms: h.window_start_ms,
            window_end_ms: h.window_end_ms,
            all_time_tokens: h.all_time_tokens,
            by_model: h.by_model,
            claude_installed: h.claude_installed,
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
}
