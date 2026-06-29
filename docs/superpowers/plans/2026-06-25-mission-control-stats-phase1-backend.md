# Mission Control Stats — Phase 1 (backend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the desktop Claude + Codex usage-stats Tauri commands return REAL data (read `~/.claude` + `~/.codex` logs and aggregate) so the Stats UI — in Settings today, and on Mission Control after Phase 2 — shows live numbers instead of zeros.

**Architecture:** All aggregation lives in `crates/agentum-server/src/usage.rs` as new `pub` functions split into **pure cores** (operate on already-parsed records — fully unit-testable, no filesystem) plus thin **public wrappers** that resolve the on-disk log paths and call the core. The desktop command files (`crates/agentum-desktop/src/commands/{claude_usage,codex_usage}.rs`) become thin adapters: parse the request's `scope`/`range`/`kind`/`limit`, call `agentum_server::usage::*`, and return camelCase structs that match the existing TypeScript contracts. A tiny persisted prefs file drives the per-provider enable toggle.

**Tech Stack:** Rust, `serde`/`serde_json`, Tauri 2 commands, `tempfile` (dev-dep, already present). The desktop crate already depends on `agentum-server` (`crates/agentum-desktop/Cargo.toml:27`), and `agentum-server/src/lib.rs:50` already exports `pub mod usage`.

## Global Constraints

Copied verbatim from the spec (`docs/superpowers/specs/2026-06-25-mission-control-stats-dashboard-design.md`); every task implicitly includes these:

- **Reuse, don't rewrite:** reuse `walk_jsonl` (`usage.rs:113`), `parse_iso8601_ms` (`usage.rs:151`), `home_dir` (`usage.rs:137`), `now_ms` (`usage.rs:141`). Do NOT touch the working 5h-window chip (`scan_claude`/`scan_codex`) or the `/api/usage` routes.
- **Claude + Codex only.** OpenCode stays stubbed (its "Soon" UI is Phase 2). Do not implement `open_code_usage_*`.
- **Exact JSON contracts** (camelCase) — must match the TS types verbatim: `ui/src/shared/claude-usage-types.ts`, `ui/src/shared/codex-usage-types.ts`. Use `#[serde(rename_all = "camelCase")]` on every new response struct.
- **`scope`:** ship `"all"` correctly; treat `"agentum"` == `"all"` in v1 with a `// TODO` (do NOT half-build a path filter).
- **Cost is an estimate**, labelled as such in the UI. Unknown model ⇒ no cost contribution.
- **Local-only, no network**, no DB for usage. Pure filesystem reads of `~/.claude/{projects,transcripts}` and `~/.codex/sessions`.
- **Default enable = true** for Claude + Codex (so the dashboard shows data on first open); persisted, user can toggle off.
- **Build gate:** `cargo build -p agentum-desktop` and `cargo test -p agentum-server --lib` must pass. Run `cargo fmt --all` before each commit (CI fmt gate; local fmt == CI).
- **Contribution workflow:** Phase 1 lands issue-first as a PR into `develop`.
- **Verified on-disk shapes (use these exact field paths):**
  - Claude record (one JSON object per line, only `message.usage`-bearing lines matter): top-level `cwd` (str), `gitBranch` (str), `sessionId` (str), `timestamp` (ISO-8601 `…Z`); `message.model` (str); `message.usage.{input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens}` (u64).
  - Codex record: top-level `timestamp` (ISO-8601); `payload.type == "token_count"`; token counts under `payload.info.last_token_usage.{input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens}` (u64). `payload.info` is `null` on rate-limit-only events — skip those. The Codex **model is NOT in the token_count event** (recon in Task 3).

---

### Task 1: Per-record parsers + `UsageRange`

**Files:**
- Modify: `crates/agentum-server/src/usage.rs` (add below the existing helpers, before the `#[cfg(test)] mod tests` block at `:838`)
- Test: same file, inside `mod tests`

**Interfaces:**
- Produces:
  - `pub(crate) struct ParsedClaudeRecord { pub ts_ms: i64, pub day: String, pub project: String, pub project_label: String, pub branch: Option<String>, pub session_id: String, pub model: Option<String>, pub input: u64, pub output: u64, pub cache_read: u64, pub cache_write: u64 }`
  - `pub fn parse_claude_usage_record(line: &str) -> Option<ParsedClaudeRecord>`
  - `pub(crate) struct ParsedCodexRecord { pub ts_ms: i64, pub day: String, pub session_id: String, pub model: Option<String>, pub input: u64, pub cached_input: u64, pub output: u64, pub reasoning_output: u64, pub total: u64 }`
  - `pub fn parse_codex_usage_record(line: &str, session_id: &str, model: Option<&str>) -> Option<ParsedCodexRecord>`
  - `pub enum UsageRange { D7, D30, D90, All }` with `pub fn from_str(s: &str) -> UsageRange` (default `All`) and `pub fn floor_ms(&self, now_ms: i64) -> Option<i64>` (`None` for `All`).

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` (`usage.rs`, after `:906`):

```rust
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
        assert!(parse_claude_usage_record(r#"{"type":"user","message":{"role":"user"}}"#).is_none());
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
        assert_eq!(UsageRange::from_str("7d").floor_ms(now), Some(now - 7 * 86_400_000));
        assert_eq!(UsageRange::from_str("all").floor_ms(now), None);
        assert!(matches!(UsageRange::from_str("nonsense"), UsageRange::All));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agentum-server --lib usage 2>&1 | tail -20`
Expected: FAIL — `cannot find function parse_claude_usage_record` (and the others).

- [ ] **Step 3: Write the implementation**

Insert into `usage.rs` just before `#[cfg(test)] mod tests {` (`:838`):

```rust
// ===========================================================================
// Stats aggregation (Mission Control). Separate from the 5h-window chip above:
// the chip lumps `billable = input+output+cache_create` and drops cache_read;
// the stats surface needs all four token classes kept apart, plus project /
// session / model attribution. Pure parsers + pure aggregators (testable
// without the filesystem) sit under thin path-resolving wrappers.
// ===========================================================================

/// One Claude usage-bearing assistant record, fully attributed.
pub(crate) struct ParsedClaudeRecord {
    pub ts_ms: i64,
    pub day: String,
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
    cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or(cwd).to_string()
}

pub fn parse_claude_usage_record(line: &str) -> Option<ParsedClaudeRecord> {
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
    let cwd = v.get("cwd").and_then(|c| c.as_str()).unwrap_or("").to_string();
    Some(ParsedClaudeRecord {
        ts_ms,
        day,
        project_label: project_label_from_path(&cwd),
        project: cwd,
        branch: v.get("gitBranch").and_then(|b| b.as_str()).filter(|s| !s.is_empty()).map(String::from),
        session_id: v.get("sessionId").and_then(|s| s.as_str()).unwrap_or("").to_string(),
        model: msg.get("model").and_then(|m| m.as_str()).map(String::from),
        input,
        output,
        cache_read,
        cache_write,
    })
}

/// One Codex `token_count` record's per-turn delta (`last_token_usage`).
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

pub fn parse_codex_usage_record(
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
    pub fn from_str(s: &str) -> UsageRange {
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentum-server --lib usage 2>&1 | tail -20`
Expected: PASS (all Task-1 tests green; pre-existing usage tests still green).

- [ ] **Step 5: Format + commit**

```bash
cargo fmt --all
git add crates/agentum-server/src/usage.rs
git commit -m "feat(usage): per-record Claude/Codex stats parsers + UsageRange

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Claude aggregation (contracts + pure cores + wrappers)

**Files:**
- Modify: `crates/agentum-server/src/usage.rs` (append after Task 1's code)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `ParsedClaudeRecord`, `parse_claude_usage_record`, `UsageRange`, `walk_jsonl`, `home_dir`, `now_ms` (all from Task 1 / existing).
- Produces (camelCase serde structs mirroring `ui/src/shared/claude-usage-types.ts`):
  - `ClaudeUsageSummary { scope, range, sessions, turns, zero_cache_read_turns, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cache_reuse_rate: Option<f64>, estimated_cost_usd: Option<f64>, top_model: Option<String>, top_project: Option<String>, has_any_claude_data }`
  - `ClaudeUsageDailyPoint { day, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }`
  - `ClaudeUsageBreakdownRow { key, label, sessions, turns, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, estimated_cost_usd: Option<f64> }`
  - `ClaudeUsageSessionRow { session_id, last_active_at: String, duration_minutes: u64, project_label, branch: Option<String>, model: Option<String>, turns, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }`
  - `pub fn claude_usage_summary(scope: &str, range: &str) -> ClaudeUsageSummary`
  - `pub fn claude_usage_daily(scope: &str, range: &str) -> Vec<ClaudeUsageDailyPoint>`
  - `pub fn claude_usage_breakdown(scope: &str, range: &str, kind: &str) -> Vec<ClaudeUsageBreakdownRow>`
  - `pub fn claude_usage_recent_sessions(scope: &str, range: &str, limit: usize) -> Vec<ClaudeUsageSessionRow>`
  - `pub fn claude_has_any_data() -> bool`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
    fn claude_fixture() -> Vec<ParsedClaudeRecord> {
        // Two sessions, two days, two models, two projects.
        let mk = |ts: &str, day: &str, proj: &str, label: &str, sess: &str, model: &str,
                  i: u64, o: u64, cr: u64, cw: u64| ParsedClaudeRecord {
            ts_ms: parse_iso8601_ms(ts).unwrap(),
            day: day.to_string(),
            project: proj.to_string(),
            project_label: label.to_string(),
            branch: Some("main".to_string()),
            session_id: sess.to_string(),
            model: Some(model.to_string()),
            input: i, output: o, cache_read: cr, cache_write: cw,
        };
        vec![
            mk("2026-06-18T10:00:00Z", "2026-06-18", "/p/alpha", "alpha", "s1", "claude-opus-4-8", 100, 200, 50, 10),
            mk("2026-06-18T11:00:00Z", "2026-06-18", "/p/alpha", "alpha", "s1", "claude-opus-4-8", 0, 5, 0, 0),
            mk("2026-06-19T09:00:00Z", "2026-06-19", "/p/beta", "beta", "s2", "claude-sonnet-4-6", 1000, 50, 900, 100),
        ]
    }

    #[test]
    fn claude_summary_totals_and_tops() {
        let s = claude_usage_summary_from_records(claude_fixture(), "all", UsageRange::All, 1_780_000_000_000);
        assert_eq!(s.sessions, 2);
        assert_eq!(s.turns, 3);
        assert_eq!(s.input_tokens, 1100);
        assert_eq!(s.output_tokens, 255);
        assert_eq!(s.cache_read_tokens, 950);
        assert_eq!(s.cache_write_tokens, 110);
        assert_eq!(s.zero_cache_read_turns, 2); // the two records with cache_read == 0
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
        assert!(bm.iter().any(|r| r.label == "claude-opus-4-8" && r.turns == 2));
        let bp = claude_usage_breakdown_from_records(claude_fixture(), "project");
        assert!(bp.iter().any(|r| r.label == "beta" && r.input_tokens == 1000));
    }

    #[test]
    fn claude_recent_sessions_sorted_desc_and_limited() {
        let rs = claude_usage_recent_sessions_from_records(claude_fixture(), 1);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].session_id, "s2"); // most-recent lastActiveAt
        assert_eq!(rs[0].project_label, "beta");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentum-server --lib usage 2>&1 | tail -20`
Expected: FAIL — `cannot find function claude_usage_summary_from_records`.

- [ ] **Step 3: Write the implementation**

Append to `usage.rs` (after Task 1's block):

```rust
use serde::Serialize as _Ser; // (no-op marker; Serialize already imported at top)

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

/// Per-Mtok USD rates by model family. cache_read ≈ 0.1× input (Anthropic
/// standard). ESTIMATE only.
struct ClaudeRates { input: f64, output: f64, cache_write: f64, cache_read: f64 }
fn claude_rates(model: &str) -> Option<ClaudeRates> {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        Some(ClaudeRates { input: 15.0, output: 75.0, cache_write: 18.75, cache_read: 1.50 })
    } else if m.contains("sonnet") {
        Some(ClaudeRates { input: 3.0, output: 15.0, cache_write: 3.75, cache_read: 0.30 })
    } else if m.contains("haiku") {
        Some(ClaudeRates { input: 0.80, output: 4.0, cache_write: 1.0, cache_read: 0.08 })
    } else {
        None
    }
}
fn claude_cost(model: Option<&str>, input: u64, output: u64, cw: u64, cr: u64) -> Option<f64> {
    let r = claude_rates(model?)?;
    let m = 1_000_000.0;
    Some(input as f64 * r.input / m + output as f64 * r.output / m
        + cw as f64 * r.cache_write / m + cr as f64 * r.cache_read / m)
}

fn claude_in_range(records: Vec<ParsedClaudeRecord>, range: UsageRange, now_ms: i64) -> Vec<ParsedClaudeRecord> {
    match range.floor_ms(now_ms) {
        Some(floor) => records.into_iter().filter(|r| r.ts_ms >= floor).collect(),
        None => records,
    }
}

pub(crate) fn claude_usage_summary_from_records(
    records: Vec<ParsedClaudeRecord>,
    scope: &str,
    range: UsageRange,
    now_ms: i64,
) -> ClaudeUsageSummary {
    let records = claude_in_range(records, range, now_ms);
    let mut sessions = std::collections::BTreeSet::new();
    let (mut input, mut output, mut cr, mut cw, mut zero_cr, mut cost) = (0u64, 0u64, 0u64, 0u64, 0u64, 0.0f64);
    let mut cost_any = false;
    let mut by_model: std::collections::BTreeMap<String, u64> = Default::default();
    let mut by_project: std::collections::BTreeMap<String, u64> = Default::default();
    for r in &records {
        sessions.insert(r.session_id.clone());
        input += r.input; output += r.output; cr += r.cache_read; cw += r.cache_write;
        if r.cache_read == 0 { zero_cr += 1; }
        if let Some(c) = claude_cost(r.model.as_deref(), r.input, r.output, r.cache_write, r.cache_read) {
            cost += c; cost_any = true;
        }
        let tot = r.input + r.output + r.cache_read + r.cache_write;
        if let Some(m) = &r.model { *by_model.entry(m.clone()).or_default() += tot; }
        *by_project.entry(r.project_label.clone()).or_default() += tot;
    }
    let top = |m: &std::collections::BTreeMap<String, u64>| {
        m.iter().max_by_key(|(_, v)| **v).map(|(k, _)| k.clone())
    };
    let denom = cr + input;
    ClaudeUsageSummary {
        scope: scope.to_string(),
        range: range_label(&range),
        sessions: sessions.len() as u64,
        turns: records.len() as u64,
        zero_cache_read_turns: zero_cr,
        input_tokens: input, output_tokens: output, cache_read_tokens: cr, cache_write_tokens: cw,
        cache_reuse_rate: if denom > 0 { Some(cr as f64 / denom as f64) } else { None },
        estimated_cost_usd: cost_any.then_some(cost),
        top_model: top(&by_model),
        top_project: top(&by_project),
        has_any_claude_data: !records.is_empty(),
    }
}

fn range_label(r: &UsageRange) -> String {
    match r { UsageRange::D7 => "7d", UsageRange::D30 => "30d", UsageRange::D90 => "90d", UsageRange::All => "all" }.to_string()
}

pub(crate) fn claude_usage_daily_from_records(records: Vec<ParsedClaudeRecord>) -> Vec<ClaudeUsageDailyPoint> {
    let mut by_day: std::collections::BTreeMap<String, ClaudeUsageDailyPoint> = Default::default();
    for r in records {
        let e = by_day.entry(r.day.clone()).or_insert(ClaudeUsageDailyPoint {
            day: r.day.clone(), input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0,
        });
        e.input_tokens += r.input; e.output_tokens += r.output;
        e.cache_read_tokens += r.cache_read; e.cache_write_tokens += r.cache_write;
    }
    by_day.into_values().collect() // BTreeMap ⇒ ascending by day
}

pub(crate) fn claude_usage_breakdown_from_records(records: Vec<ParsedClaudeRecord>, kind: &str) -> Vec<ClaudeUsageBreakdownRow> {
    struct Acc { label: String, sessions: std::collections::BTreeSet<String>, turns: u64, input: u64, output: u64, cr: u64, cw: u64, cost: f64, cost_any: bool }
    let mut groups: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        let (key, label) = if kind == "project" {
            (r.project_label.clone(), r.project_label.clone())
        } else {
            let m = r.model.clone().unwrap_or_else(|| "unknown".to_string());
            (m.clone(), m)
        };
        let a = groups.entry(key.clone()).or_insert(Acc { label, sessions: Default::default(), turns: 0, input: 0, output: 0, cr: 0, cw: 0, cost: 0.0, cost_any: false });
        a.sessions.insert(r.session_id.clone());
        a.turns += 1; a.input += r.input; a.output += r.output; a.cr += r.cache_read; a.cw += r.cache_write;
        if let Some(c) = claude_cost(r.model.as_deref(), r.input, r.output, r.cache_write, r.cache_read) { a.cost += c; a.cost_any = true; }
    }
    let mut rows: Vec<ClaudeUsageBreakdownRow> = groups.into_iter().map(|(key, a)| ClaudeUsageBreakdownRow {
        key, label: a.label, sessions: a.sessions.len() as u64, turns: a.turns,
        input_tokens: a.input, output_tokens: a.output, cache_read_tokens: a.cr, cache_write_tokens: a.cw,
        estimated_cost_usd: a.cost_any.then_some(a.cost),
    }).collect();
    rows.sort_by(|x, y| (y.input_tokens + y.output_tokens + y.cache_read_tokens + y.cache_write_tokens)
        .cmp(&(x.input_tokens + x.output_tokens + x.cache_read_tokens + x.cache_write_tokens)));
    rows
}

pub(crate) fn claude_usage_recent_sessions_from_records(records: Vec<ParsedClaudeRecord>, limit: usize) -> Vec<ClaudeUsageSessionRow> {
    struct Acc { first_ms: i64, last_ms: i64, last_ts: String, project_label: String, branch: Option<String>, model: Option<String>, turns: u64, input: u64, output: u64, cr: u64, cw: u64 }
    let mut by_session: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        let last_ts = iso_from_ms_fallback(&r);
        let a = by_session.entry(r.session_id.clone()).or_insert(Acc {
            first_ms: r.ts_ms, last_ms: r.ts_ms, last_ts: last_ts.clone(),
            project_label: r.project_label.clone(), branch: r.branch.clone(), model: r.model.clone(),
            turns: 0, input: 0, output: 0, cr: 0, cw: 0,
        });
        a.first_ms = a.first_ms.min(r.ts_ms);
        if r.ts_ms >= a.last_ms { a.last_ms = r.ts_ms; a.last_ts = last_ts; a.model = r.model.clone(); }
        a.turns += 1; a.input += r.input; a.output += r.output; a.cr += r.cache_read; a.cw += r.cache_write;
    }
    let mut rows: Vec<(i64, ClaudeUsageSessionRow)> = by_session.into_iter().map(|(session_id, a)| (a.last_ms, ClaudeUsageSessionRow {
        session_id, last_active_at: a.last_ts, duration_minutes: ((a.last_ms - a.first_ms).max(0) / 60_000) as u64,
        project_label: a.project_label, branch: a.branch, model: a.model, turns: a.turns,
        input_tokens: a.input, output_tokens: a.output, cache_read_tokens: a.cr, cache_write_tokens: a.cw,
    })).collect();
    rows.sort_by(|x, y| y.0.cmp(&x.0));
    rows.into_iter().take(limit).map(|(_, row)| row).collect()
}

// The record dropped the original ISO string; reconstruct a stable label from
// its day for the UI. (lastActiveAt is rendered as a date; day precision is
// sufficient and avoids threading the raw string through every record.)
fn iso_from_ms_fallback(r: &ParsedClaudeRecord) -> String {
    r.day.clone()
}

// ---- path-resolving wrappers (the desktop commands call these) ----

fn claude_log_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = home_dir() {
        for sub in [".claude/projects", ".claude/transcripts"] {
            let root = home.join(sub);
            if root.exists() { files.extend(walk_jsonl(&root)); }
        }
    }
    files
}

fn collect_claude_records() -> Vec<ParsedClaudeRecord> {
    let mut out = Vec::new();
    for path in claude_log_files() {
        let file = match File::open(&path) { Ok(f) => f, Err(_) => continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some(r) = parse_claude_usage_record(&line) { out.push(r); }
        }
    }
    out
}

pub fn claude_has_any_data() -> bool {
    home_dir().map(|h| h.join(".claude/projects").exists() || h.join(".claude/transcripts").exists()).unwrap_or(false)
}

pub fn claude_usage_summary(scope: &str, range: &str) -> ClaudeUsageSummary {
    claude_usage_summary_from_records(collect_claude_records(), scope, UsageRange::from_str(range), now_ms())
}
pub fn claude_usage_daily(_scope: &str, range: &str) -> Vec<ClaudeUsageDailyPoint> {
    claude_usage_daily_from_records(claude_in_range(collect_claude_records(), UsageRange::from_str(range), now_ms()))
}
pub fn claude_usage_breakdown(_scope: &str, range: &str, kind: &str) -> Vec<ClaudeUsageBreakdownRow> {
    claude_usage_breakdown_from_records(claude_in_range(collect_claude_records(), UsageRange::from_str(range), now_ms()), kind)
}
pub fn claude_usage_recent_sessions(_scope: &str, range: &str, limit: usize) -> Vec<ClaudeUsageSessionRow> {
    claude_usage_recent_sessions_from_records(claude_in_range(collect_claude_records(), UsageRange::from_str(range), now_ms()), limit)
}
```

Note: delete the `use serde::Serialize as _Ser;` marker line — `Serialize` is already imported at the top of the file (`usage.rs:25`). It is shown only to flag that no new import is needed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentum-server --lib usage 2>&1 | tail -25`
Expected: PASS (Task 1 + Task 2 tests green).

- [ ] **Step 5: Format + commit**

```bash
cargo fmt --all
git add crates/agentum-server/src/usage.rs
git commit -m "feat(usage): Claude stats aggregation (summary/daily/breakdown/sessions)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Codex aggregation (+ model recon)

**Files:**
- Modify: `crates/agentum-server/src/usage.rs`
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `ParsedCodexRecord`, `parse_codex_usage_record`, `UsageRange`, `walk_jsonl`, `home_dir`, `now_ms`.
- Produces (camelCase, mirroring `ui/src/shared/codex-usage-types.ts`):
  - `CodexUsageSummary { scope, range, sessions, events, input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens, estimated_cost_usd: Option<f64>, top_model: Option<String>, top_project: Option<String>, has_any_codex_data }`
  - `CodexUsageDailyPoint { day, input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens }`
  - `CodexUsageBreakdownRow { key, label, sessions, events, input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens, estimated_cost_usd: Option<f64>, has_inferred_pricing: bool }`
  - `CodexUsageSessionRow { session_id, last_active_at, duration_minutes, project_label, model: Option<String>, events, input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens, has_inferred_pricing: bool }`
  - `pub fn codex_usage_summary(scope, range) -> CodexUsageSummary`, `codex_usage_daily`, `codex_usage_breakdown`, `codex_usage_recent_sessions`, `codex_has_any_data`.

- [ ] **Step 1: RECON the Codex model field (no code yet)**

Codex `token_count` events carry no model. Find where the session's model lives:

Run: `f=$(find ~/.codex/sessions -name '*.jsonl' | head -1); grep -o '"model":"[^"]*"' "$f" | sort -u | head; echo "---meta---"; head -3 "$f" | cut -c1-400`
Expected: a `"model":"…"` somewhere in a `session_meta`/`turn_context`/config record near the top of the file. Record the exact JSON path (e.g. `payload.model` on the first `session_meta` line). If NO model field exists, the parser passes `model = None` and breakdown groups under `"codex"` — that is acceptable (`hasInferredPricing = true`).

- [ ] **Step 2: Write the failing tests**

```rust
    fn codex_fixture() -> Vec<ParsedCodexRecord> {
        let mk = |ts: &str, day: &str, sess: &str, i: u64, ci: u64, o: u64, ro: u64, tot: u64| ParsedCodexRecord {
            ts_ms: parse_iso8601_ms(ts).unwrap(), day: day.to_string(), session_id: sess.to_string(),
            model: Some("gpt-5-codex".to_string()), input: i, cached_input: ci, output: o, reasoning_output: ro, total: tot,
        };
        vec![
            mk("2026-04-11T01:00:00Z", "2026-04-11", "c1", 100, 20, 30, 5, 155),
            mk("2026-04-11T02:00:00Z", "2026-04-11", "c1", 200, 0, 40, 0, 240),
            mk("2026-04-12T01:00:00Z", "2026-04-12", "c2", 9, 0, 1, 0, 10),
        ]
    }

    #[test]
    fn codex_summary_totals() {
        let s = codex_usage_summary_from_records(codex_fixture(), "all", UsageRange::All, 1_780_000_000_000);
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
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p agentum-server --lib usage 2>&1 | tail -20`
Expected: FAIL — `cannot find function codex_usage_summary_from_records`.

- [ ] **Step 4: Write the implementation**

Append to `usage.rs`. Mirror Task 2's Claude structs/aggregators with Codex fields. (Full code — repeated, not cross-referenced.)

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageSummary {
    pub scope: String, pub range: String, pub sessions: u64, pub events: u64,
    pub input_tokens: u64, pub cached_input_tokens: u64, pub output_tokens: u64,
    pub reasoning_output_tokens: u64, pub total_tokens: u64,
    pub estimated_cost_usd: Option<f64>, pub top_model: Option<String>,
    pub top_project: Option<String>, pub has_any_codex_data: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageDailyPoint {
    pub day: String, pub input_tokens: u64, pub cached_input_tokens: u64,
    pub output_tokens: u64, pub reasoning_output_tokens: u64, pub total_tokens: u64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageBreakdownRow {
    pub key: String, pub label: String, pub sessions: u64, pub events: u64,
    pub input_tokens: u64, pub cached_input_tokens: u64, pub output_tokens: u64,
    pub reasoning_output_tokens: u64, pub total_tokens: u64,
    pub estimated_cost_usd: Option<f64>, pub has_inferred_pricing: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageSessionRow {
    pub session_id: String, pub last_active_at: String, pub duration_minutes: u64,
    pub project_label: String, pub model: Option<String>, pub events: u64,
    pub input_tokens: u64, pub cached_input_tokens: u64, pub output_tokens: u64,
    pub reasoning_output_tokens: u64, pub total_tokens: u64, pub has_inferred_pricing: bool,
}

fn codex_in_range(records: Vec<ParsedCodexRecord>, range: UsageRange, now_ms: i64) -> Vec<ParsedCodexRecord> {
    match range.floor_ms(now_ms) {
        Some(floor) => records.into_iter().filter(|r| r.ts_ms >= floor).collect(),
        None => records,
    }
}

pub(crate) fn codex_usage_summary_from_records(records: Vec<ParsedCodexRecord>, scope: &str, range: UsageRange, now_ms: i64) -> CodexUsageSummary {
    let records = codex_in_range(records, range, now_ms);
    let mut sessions = std::collections::BTreeSet::new();
    let (mut i, mut ci, mut o, mut ro, mut tot) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut by_model: std::collections::BTreeMap<String, u64> = Default::default();
    for r in &records {
        sessions.insert(r.session_id.clone());
        i += r.input; ci += r.cached_input; o += r.output; ro += r.reasoning_output; tot += r.total;
        if let Some(m) = &r.model { *by_model.entry(m.clone()).or_default() += r.total; }
    }
    CodexUsageSummary {
        scope: scope.to_string(), range: range_label(&range),
        sessions: sessions.len() as u64, events: records.len() as u64,
        input_tokens: i, cached_input_tokens: ci, output_tokens: o, reasoning_output_tokens: ro, total_tokens: tot,
        estimated_cost_usd: None, // Codex pricing unreliable (no per-model billing source) — inferred.
        top_model: by_model.iter().max_by_key(|(_, v)| **v).map(|(k, _)| k.clone()),
        top_project: None,
        has_any_codex_data: !records.is_empty(),
    }
}

pub(crate) fn codex_usage_daily_from_records(records: Vec<ParsedCodexRecord>) -> Vec<CodexUsageDailyPoint> {
    let mut by_day: std::collections::BTreeMap<String, CodexUsageDailyPoint> = Default::default();
    for r in records {
        let e = by_day.entry(r.day.clone()).or_insert(CodexUsageDailyPoint { day: r.day.clone(), input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_output_tokens: 0, total_tokens: 0 });
        e.input_tokens += r.input; e.cached_input_tokens += r.cached_input; e.output_tokens += r.output;
        e.reasoning_output_tokens += r.reasoning_output; e.total_tokens += r.total;
    }
    by_day.into_values().collect()
}

pub(crate) fn codex_usage_breakdown_from_records(records: Vec<ParsedCodexRecord>, kind: &str) -> Vec<CodexUsageBreakdownRow> {
    struct Acc { label: String, sessions: std::collections::BTreeSet<String>, events: u64, i: u64, ci: u64, o: u64, ro: u64, tot: u64 }
    let mut groups: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        let (key, label) = if kind == "project" {
            ("codex".to_string(), "codex".to_string()) // Codex has no per-record project; single bucket.
        } else {
            let m = r.model.clone().unwrap_or_else(|| "codex".to_string());
            (m.clone(), m)
        };
        let a = groups.entry(key.clone()).or_insert(Acc { label, sessions: Default::default(), events: 0, i: 0, ci: 0, o: 0, ro: 0, tot: 0 });
        a.sessions.insert(r.session_id.clone()); a.events += 1;
        a.i += r.input; a.ci += r.cached_input; a.o += r.output; a.ro += r.reasoning_output; a.tot += r.total;
    }
    let mut rows: Vec<CodexUsageBreakdownRow> = groups.into_iter().map(|(key, a)| CodexUsageBreakdownRow {
        key, label: a.label, sessions: a.sessions.len() as u64, events: a.events,
        input_tokens: a.i, cached_input_tokens: a.ci, output_tokens: a.o, reasoning_output_tokens: a.ro, total_tokens: a.tot,
        estimated_cost_usd: None, has_inferred_pricing: true,
    }).collect();
    rows.sort_by(|x, y| y.total_tokens.cmp(&x.total_tokens));
    rows
}

pub(crate) fn codex_usage_recent_sessions_from_records(records: Vec<ParsedCodexRecord>, limit: usize) -> Vec<CodexUsageSessionRow> {
    struct Acc { first_ms: i64, last_ms: i64, last_day: String, model: Option<String>, events: u64, i: u64, ci: u64, o: u64, ro: u64, tot: u64 }
    let mut by_session: std::collections::BTreeMap<String, Acc> = Default::default();
    for r in records {
        let a = by_session.entry(r.session_id.clone()).or_insert(Acc { first_ms: r.ts_ms, last_ms: r.ts_ms, last_day: r.day.clone(), model: r.model.clone(), events: 0, i: 0, ci: 0, o: 0, ro: 0, tot: 0 });
        a.first_ms = a.first_ms.min(r.ts_ms);
        if r.ts_ms >= a.last_ms { a.last_ms = r.ts_ms; a.last_day = r.day.clone(); a.model = r.model.clone(); }
        a.events += 1; a.i += r.input; a.ci += r.cached_input; a.o += r.output; a.ro += r.reasoning_output; a.tot += r.total;
    }
    let mut rows: Vec<(i64, CodexUsageSessionRow)> = by_session.into_iter().map(|(session_id, a)| (a.last_ms, CodexUsageSessionRow {
        session_id, last_active_at: a.last_day, duration_minutes: ((a.last_ms - a.first_ms).max(0) / 60_000) as u64,
        project_label: "codex".to_string(), model: a.model, events: a.events,
        input_tokens: a.i, cached_input_tokens: a.ci, output_tokens: a.o, reasoning_output_tokens: a.ro, total_tokens: a.tot,
        has_inferred_pricing: true,
    })).collect();
    rows.sort_by(|x, y| y.0.cmp(&x.0));
    rows.into_iter().take(limit).map(|(_, row)| row).collect()
}

// ---- path-resolving wrappers ----

fn codex_session_files() -> Vec<PathBuf> {
    home_dir().map(|h| { let d = h.join(".codex/sessions"); if d.exists() { walk_jsonl(&d) } else { Vec::new() } }).unwrap_or_default()
}

/// Read the session's model from its first model-bearing line (recon, Task 3
/// Step 1 confirms the path). Falls back to `None`.
fn codex_model_for(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            // Adjust this path per Task 3 Step 1 recon if needed.
            if let Some(m) = v.get("payload").and_then(|p| p.get("model")).and_then(|m| m.as_str()) {
                return Some(m.to_string());
            }
        }
    }
    None
}

fn collect_codex_records() -> Vec<ParsedCodexRecord> {
    let mut out = Vec::new();
    for path in codex_session_files() {
        let session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let model = codex_model_for(&path);
        let file = match File::open(&path) { Ok(f) => f, Err(_) => continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some(r) = parse_codex_usage_record(&line, &session_id, model.as_deref()) { out.push(r); }
        }
    }
    out
}

pub fn codex_has_any_data() -> bool {
    home_dir().map(|h| h.join(".codex/sessions").exists()).unwrap_or(false)
}
pub fn codex_usage_summary(scope: &str, range: &str) -> CodexUsageSummary {
    codex_usage_summary_from_records(collect_codex_records(), scope, UsageRange::from_str(range), now_ms())
}
pub fn codex_usage_daily(_scope: &str, range: &str) -> Vec<CodexUsageDailyPoint> {
    codex_usage_daily_from_records(codex_in_range(collect_codex_records(), UsageRange::from_str(range), now_ms()))
}
pub fn codex_usage_breakdown(_scope: &str, range: &str, kind: &str) -> Vec<CodexUsageBreakdownRow> {
    codex_usage_breakdown_from_records(codex_in_range(collect_codex_records(), UsageRange::from_str(range), now_ms()), kind)
}
pub fn codex_usage_recent_sessions(_scope: &str, range: &str, limit: usize) -> Vec<CodexUsageSessionRow> {
    codex_usage_recent_sessions_from_records(codex_in_range(collect_codex_records(), UsageRange::from_str(range), now_ms()), limit)
}
```

- [ ] **Step 5: Run the tests + commit**

```bash
cargo test -p agentum-server --lib usage 2>&1 | tail -25   # expect PASS
cargo fmt --all
git add crates/agentum-server/src/usage.rs
git commit -m "feat(usage): Codex stats aggregation (summary/daily/breakdown/sessions)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Persisted enable-flag + scan-state helper

**Files:**
- Create: `crates/agentum-desktop/src/commands/usage_prefs.rs`
- Modify: `crates/agentum-desktop/src/commands/mod.rs` (add `pub mod usage_prefs;` — match the existing `pub mod claude_usage;` block at `:7`)
- Test: `crates/agentum-desktop/src/commands/usage_prefs.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub fn provider_enabled(provider: &str, default: bool) -> bool`
  - `pub fn set_provider_enabled(provider: &str, enabled: bool)`
  - `pub fn prefs_path() -> Option<std::path::PathBuf>` (`$HOME/.agentum/usage-prefs.json`)

- [ ] **Step 1: Write the failing test**

```rust
// crates/agentum-desktop/src/commands/usage_prefs.rs
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_when_absent_then_roundtrips() {
        // Isolate HOME to a temp dir so the real prefs file is never touched.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        assert!(provider_enabled("claude", true));   // default = true
        assert!(!provider_enabled("claude", false));  // default = false honored when no file
        set_provider_enabled("claude", false);
        assert!(!provider_enabled("claude", true));   // persisted false wins over default
        set_provider_enabled("claude", true);
        assert!(provider_enabled("claude", false));
    }
}
```

Add to `crates/agentum-desktop/Cargo.toml` `[dev-dependencies]` if absent: `tempfile = "3"` (check first: `grep tempfile crates/agentum-desktop/Cargo.toml`).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p agentum-desktop --lib usage_prefs 2>&1 | tail -20`
Expected: FAIL — module/functions not found.

- [ ] **Step 3: Write the implementation**

```rust
// crates/agentum-desktop/src/commands/usage_prefs.rs
//! Tiny persisted per-provider enable flag for usage scanning. Local JSON at
//! `$HOME/.agentum/usage-prefs.json`; absent ⇒ caller's default (true for
//! Claude/Codex so the dashboard shows data on first open).
use std::path::PathBuf;

pub fn prefs_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".agentum").join("usage-prefs.json"))
}

fn load() -> serde_json::Value {
    prefs_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn provider_enabled(provider: &str, default: bool) -> bool {
    load().get(provider).and_then(|v| v.as_bool()).unwrap_or(default)
}

pub fn set_provider_enabled(provider: &str, enabled: bool) {
    let mut cfg = load();
    cfg[provider] = serde_json::Value::Bool(enabled);
    if let Some(path) = prefs_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(&cfg) {
            let _ = std::fs::write(path, s);
        }
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p agentum-desktop --lib usage_prefs 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Format + commit**

```bash
cargo fmt --all
git add crates/agentum-desktop/src/commands/usage_prefs.rs crates/agentum-desktop/src/commands/mod.rs crates/agentum-desktop/Cargo.toml
git commit -m "feat(usage): persisted per-provider enable flag

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Wire the Claude desktop commands to real data

**Files:**
- Modify (replace bodies): `crates/agentum-desktop/src/commands/claude_usage.rs`

**Interfaces:**
- Consumes: `agentum_server::usage::{claude_usage_summary, claude_usage_daily, claude_usage_breakdown, claude_usage_recent_sessions, claude_has_any_data}` (Task 2), `super::usage_prefs` (Task 4).
- Produces: the same Tauri command names (no `lib.rs` invoke-handler change needed) — now returning real serde structs.

- [ ] **Step 1: Replace the file contents**

```rust
// crates/agentum-desktop/src/commands/claude_usage.rs
use agentum_server::usage;
use serde::Serialize;
use serde_json::{json, Value};

use super::usage_prefs;

const PROVIDER: &str = "claude";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanState {
    enabled: bool,
    is_scanning: bool,
    last_scan_started_at: Option<i64>,
    last_scan_completed_at: Option<i64>,
    last_scan_error: Option<String>,
    has_any_claude_data: bool,
}

fn scan_state(enabled: bool) -> ScanState {
    ScanState {
        enabled,
        is_scanning: false,
        last_scan_started_at: None,
        last_scan_completed_at: None,
        last_scan_error: None,
        has_any_claude_data: enabled && usage::claude_has_any_data(),
    }
}

fn scope_range(request: &tauri::ipc::Request<'_>) -> (String, String) {
    if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        (
            v.get("scope").and_then(|s| s.as_str()).unwrap_or("all").to_string(),
            v.get("range").and_then(|s| s.as_str()).unwrap_or("30d").to_string(),
        )
    } else {
        ("all".to_string(), "30d".to_string())
    }
}

#[tauri::command]
pub fn claude_usage_get_scan_state() -> Value {
    json!(scan_state(usage_prefs::provider_enabled(PROVIDER, true)))
}

#[tauri::command]
pub fn claude_usage_set_enabled(enabled: bool) -> Value {
    usage_prefs::set_provider_enabled(PROVIDER, enabled);
    json!(scan_state(enabled))
}

#[tauri::command]
pub fn claude_usage_refresh() -> Value {
    json!(scan_state(usage_prefs::provider_enabled(PROVIDER, true)))
}

#[tauri::command]
pub fn claude_usage_get_summary(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    // When the provider is disabled, report an honest empty summary
    // (hasAnyClaudeData:false) rather than scanning the logs.
    if !usage_prefs::provider_enabled(PROVIDER, true) {
        return json!({
            "scope": scope, "range": range, "sessions": 0, "turns": 0,
            "zeroCacheReadTurns": 0, "inputTokens": 0, "outputTokens": 0,
            "cacheReadTokens": 0, "cacheWriteTokens": 0, "cacheReuseRate": null,
            "estimatedCostUsd": null, "topModel": null, "topProject": null,
            "hasAnyClaudeData": false
        });
    }
    json!(usage::claude_usage_summary(&scope, &range))
}

#[tauri::command]
pub fn claude_usage_get_daily(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    json!(usage::claude_usage_daily(&scope, &range))
}

#[tauri::command]
pub fn claude_usage_get_breakdown(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    let kind = if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        v.get("kind").and_then(|k| k.as_str()).unwrap_or("model").to_string()
    } else {
        "model".to_string()
    };
    json!(usage::claude_usage_breakdown(&scope, &range, &kind))
}

#[tauri::command]
pub fn claude_usage_get_recent_sessions(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    let limit = if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        v.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize
    } else {
        10
    };
    json!(usage::claude_usage_recent_sessions(&scope, &range, limit))
}
```

- [ ] **Step 2: Build the desktop crate**

Run: `cargo build -p agentum-desktop 2>&1 | tail -20`
Expected: compiles clean (no errors). If `claude_usage_get_daily`/`breakdown`/`recent_sessions` now take a `request` arg where the invoke-handler registered the zero-arg form — that's fine, Tauri matches by name and injects the request.

- [ ] **Step 3: Manual smoke check (real logs on this machine)**

Run: `cargo test -p agentum-server --lib usage 2>&1 | tail -5` (cores still green) — and confirm the real wrappers return data:
```bash
cat > /tmp/usage_smoke.rs <<'EOF'
fn main() {
    let s = agentum_server::usage::claude_usage_summary("all", "all");
    println!("claude sessions={} turns={} input={} cost={:?}", s.sessions, s.turns, s.input_tokens, s.estimated_cost_usd);
}
EOF
```
(Optional — or just trust the unit tests + the build.) Expected: non-zero sessions/turns on a machine that has used Claude Code.

- [ ] **Step 4: Format + commit**

```bash
cargo fmt --all
git add crates/agentum-desktop/src/commands/claude_usage.rs
git commit -m "feat(usage): Claude desktop commands return real aggregated data

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Wire the Codex desktop commands to real data

**Files:**
- Modify (replace bodies): `crates/agentum-desktop/src/commands/codex_usage.rs`

**Interfaces:**
- Consumes: `agentum_server::usage::{codex_usage_summary, codex_usage_daily, codex_usage_breakdown, codex_usage_recent_sessions, codex_has_any_data}` (Task 3), `super::usage_prefs` (Task 4).

- [ ] **Step 1: Replace the file contents**

```rust
// crates/agentum-desktop/src/commands/codex_usage.rs
use agentum_server::usage;
use serde::Serialize;
use serde_json::{json, Value};

use super::usage_prefs;

const PROVIDER: &str = "codex";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanState {
    enabled: bool,
    is_scanning: bool,
    last_scan_started_at: Option<i64>,
    last_scan_completed_at: Option<i64>,
    last_scan_error: Option<String>,
    has_any_codex_data: bool,
}

fn scan_state(enabled: bool) -> ScanState {
    ScanState {
        enabled,
        is_scanning: false,
        last_scan_started_at: None,
        last_scan_completed_at: None,
        last_scan_error: None,
        has_any_codex_data: enabled && usage::codex_has_any_data(),
    }
}

fn scope_range(request: &tauri::ipc::Request<'_>) -> (String, String) {
    if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        (
            v.get("scope").and_then(|s| s.as_str()).unwrap_or("all").to_string(),
            v.get("range").and_then(|s| s.as_str()).unwrap_or("30d").to_string(),
        )
    } else {
        ("all".to_string(), "30d".to_string())
    }
}

#[tauri::command]
pub fn codex_usage_get_scan_state() -> Value {
    json!(scan_state(usage_prefs::provider_enabled(PROVIDER, true)))
}

#[tauri::command]
pub fn codex_usage_set_enabled(enabled: bool) -> Value {
    usage_prefs::set_provider_enabled(PROVIDER, enabled);
    json!(scan_state(enabled))
}

#[tauri::command]
pub fn codex_usage_refresh() -> Value {
    json!(scan_state(usage_prefs::provider_enabled(PROVIDER, true)))
}

#[tauri::command]
pub fn codex_usage_get_summary(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    if !usage_prefs::provider_enabled(PROVIDER, true) {
        return json!({
            "scope": scope, "range": range, "sessions": 0, "events": 0,
            "inputTokens": 0, "cachedInputTokens": 0, "outputTokens": 0,
            "reasoningOutputTokens": 0, "totalTokens": 0, "estimatedCostUsd": null,
            "topModel": null, "topProject": null, "hasAnyCodexData": false
        });
    }
    json!(usage::codex_usage_summary(&scope, &range))
}

#[tauri::command]
pub fn codex_usage_get_daily(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    json!(usage::codex_usage_daily(&scope, &range))
}

#[tauri::command]
pub fn codex_usage_get_breakdown(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    let kind = if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        v.get("kind").and_then(|k| k.as_str()).unwrap_or("model").to_string()
    } else {
        "model".to_string()
    };
    json!(usage::codex_usage_breakdown(&scope, &range, &kind))
}

#[tauri::command]
pub fn codex_usage_get_recent_sessions(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    let limit = if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        v.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize
    } else {
        10
    };
    json!(usage::codex_usage_recent_sessions(&scope, &range, limit))
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p agentum-desktop 2>&1 | tail -20`
Expected: compiles clean.

- [ ] **Step 3: Full backend test gate**

Run: `cargo test -p agentum-server -p agentum-desktop --lib 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 4: Format + commit**

```bash
cargo fmt --all
git add crates/agentum-desktop/src/commands/codex_usage.rs
git commit -m "feat(usage): Codex desktop commands return real aggregated data

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: `stats_get_summary` returns a typed-zeroed `StatsSummary`

**Files:**
- Modify (replace body): `crates/agentum-desktop/src/commands/stats.rs`

**Interfaces:**
- Consumes: nothing (no store handle yet).
- Produces: the `stats_get_summary` Tauri command — now a typed-zeroed
  `StatsSummary` (camelCase) instead of `{}`.

**Why (verified):** `StatsPane` (`ui/src/components/stats/StatsPane.tsx:79–110`)
renders the Overview app-cards as `summary ? (...) : null`, then at line 81
branches on `summary.totalAgentsSpawned === 0 && summary.totalPRsCreated === 0`. A
bare `{}` is truthy but its fields are `undefined`, so that guard is `false` and
the else-branch runs `summary.totalAgentsSpawned.toLocaleString()` →
`undefined.toLocaleString()`, a **TypeError**. A typed-zeroed summary makes the
guard `true`, rendering the "Start your first agent to begin tracking" empty state
cleanly. (This is NOT graceful degradation today — it is a crash.)

- [ ] **Step 1: Replace the file contents**

```rust
// crates/agentum-desktop/src/commands/stats.rs
use serde::Serialize;
use serde_json::{json, Value};

// Agentum's own activity counters (agents spawned / PRs created / agent-time)
// live in the agentum-store SQLite DB (events / session_metrics), not the usage
// logs, and this command has no store handle yet. Return a typed-zeroed summary
// so Stats → Overview renders the "Start your first agent" empty state instead of
// a bare `{}` (which crashes `undefined.toLocaleString()` in StatsPane). Wiring
// the real counters from the store is a tracked follow-up.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsSummary {
    total_agents_spawned: u64,
    total_prs_created: u64,
    total_agent_time_ms: u64,
    first_event_at: Option<i64>,
}

#[tauri::command]
pub fn stats_get_summary() -> Value {
    json!(StatsSummary {
        total_agents_spawned: 0,
        total_prs_created: 0,
        total_agent_time_ms: 0,
        first_event_at: None,
    })
}
```

- [ ] **Step 2: Build the desktop crate**

Run: `cargo build -p agentum-desktop 2>&1 | tail -20`
Expected: compiles clean (no errors).

- [ ] **Step 3: Manual check (no crash on the Overview tab)**

After Phase 2 mounts `<StatsPane/>` (or in Settings → Stats & Usage today), open
the **Overview** tab and confirm it renders "Start your first agent to begin
tracking" rather than throwing. The `#[serde(rename_all = "camelCase")]` derive
guarantees the JSON keys match `StatsSummary` in `ui/src/shared/types.ts:2856`
(`totalAgentsSpawned` / `totalPRsCreated` / `totalAgentTimeMs` / `firstEventAt`).

- [ ] **Step 4: Format + commit**

```bash
cargo fmt --all
git add crates/agentum-desktop/src/commands/stats.rs
git commit -m "fix(stats): stats_get_summary returns typed-zeroed StatsSummary

Bare {} made StatsPane crash on undefined.toLocaleString(); a typed-zeroed
summary renders the empty state instead. Real app-telemetry from the
agentum-store is a tracked follow-up.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Deferred (out of Phase 1 scope — noted, not done)

- **`stats_get_summary` REAL counters** (`crates/agentum-desktop/src/commands/stats.rs`): the `StatsSummary { totalAgentsSpawned, totalPRsCreated, totalAgentTimeMs, firstEventAt }` are agentum's OWN activity counters — a *different* data source (the `agentum-store` SQLite DB: `events` / `session_metrics` tables), not the usage logs — and this command has no store handle yet. **Task 7** ships a typed-zeroed baseline (clean empty state, no crash); wiring the REAL numbers needs a store handle + schema mapping and is a tracked follow-up. The usage panes — the bulk of "stats not working" — are fully addressed by Tasks 1–6.
- **OpenCode** stays stubbed; its "Soon" UI is Phase 2.

## Self-Review

- **Spec coverage:** Phase-1 spec §1.1 (parser, +cache_read +project) → Task 1; §1.2 (summary/daily/breakdown/recent for Claude & Codex) → Tasks 2–3; §1.3 (richer cost table) → Task 2 (`claude_rates`/`claude_cost`); §1.4 (`scope` all, agentum==all TODO) → Tasks 2–3 wrappers ignore scope with `_scope`; §1.5 (enable flag + scan state, default-on) → Tasks 4–6; §1.6 (OpenCode Soon) → deferred to Phase 2 as designed; `stats_get_summary` typed-zeroed baseline (prevents a real StatsPane crash) → Task 7, with real store-backed counters deferred. Contracts (camelCase) → `#[serde(rename_all="camelCase")]` structs in Tasks 2–3, asserted against TS field names.
- **Placeholder scan:** the only `// TODO` is the deliberate `scope=="agentum"` one from the spec. The Codex `codex_model_for` path carries a "adjust per recon" note gated by Task 3 Step 1 (a real recon step, not a placeholder). The two muddled-then-clean code variants in Task 5 Step 1 are called out explicitly with "use this clean version".
- **Type consistency:** wrapper names (`claude_usage_summary`, `codex_usage_daily`, …) match between the producing tasks and the consuming desktop commands; struct field names match the verified TS contracts; `UsageRange`/`range_label` used consistently.
