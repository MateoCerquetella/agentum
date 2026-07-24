# Architecture Notes — 001 Claude account usage readout

## Current state (what already exists)

- **Server scan** `crates/agentum-server/src/usage.rs`: `scan_claude()` sums
  transcript tokens into `ClaudeUsageSnapshot { window_tokens, resets,
  all_time_tokens }`. **No cost, no real plan %** — the percent is a
  client-side guess of `window_tokens` vs. a *configured tier*. The Codex
  path already reads a real `used_percent` + `resets_at` + `plan_type` from
  OpenAI's `rate_limits` embedded in `~/.codex` transcripts.
- **Routes** `crates/agentum-server/src/routes/usage.rs`: `/api/usage`,
  `/api/usage/claude`, `/api/usage/codex` — on-demand scan; dashboard polls 60s.
- **TUI panel** `crates/agentum-cli/src/commands/terminal/ui.rs::draw_usage_panel`
  — the bottom-left "Usage" panel (10 rows off the tree column), listing
  running sessions by tokens + `cost_usd`.
- **Cost plumbing** `agentum-core::Session.cost_usd: Option<f64>` exists but is
  **never populated** (always `None` → em-dash).
- **TUI prefs** `crates/agentum-cli/src/commands/terminal/prefs.rs::Prefs` — no
  usage-refresh key yet.

---

## Components / files to touch

1. **`crates/agentum-server/src/usage.rs`** (+ small submodules):
   - `usage/claude_oauth.rs` — read the Claude OAuth token
     (`~/.claude/.credentials.json` → `claudeAiOauth.accessToken`; env
     `CLAUDE_CODE_OAUTH_TOKEN`; macOS keychain), then
     `GET https://api.anthropic.com/api/oauth/usage` (Bearer). Parse
     `five_hour.utilization`, `seven_day.utilization`, `resets_at`.
   - `usage/pricing.rs` — static per-model input/output/cache price table →
     `est_cost_usd` from the scanned token counts.
   - `usage/claude_cli.rs` — fallback: run the `claude` CLI plan-usage command
     in a PTY, parse `NN% (used|left)`.
   - Extend `ClaudeUsageSnapshot` with **additive `Option` fields**:
     `limit_pct` (= `max(five_hour, seven_day)`), `five_hour_pct`,
     `seven_day_pct`, `resets_at_ms`, `est_cost_usd`, `source`
     (`oauth | cli | scan`).
2. **`crates/agentum-server/src/routes/usage.rs`** — no new route;
   `/api/usage/claude` now returns the enriched snapshot. Add a server-side
   cache (≥30s TTL) so N clients don't multiply OAuth calls.
3. **`crates/agentum-cli/src/commands/terminal/api.rs`** — client method to GET
   `/api/usage/claude` into a CLI-side mirror struct (new fields `Option`, so
   older daemons deserialize fine).
4. **`crates/agentum-cli/src/commands/terminal/app.rs`** — poll loop driven by
   `prefs.usage_refresh_secs`; stash the latest snapshot in `App` state.
5. **`…/ui.rs::draw_usage_panel`** — add a header line, e.g.
   `est $12.40 · 2.1M tok · 🟡 82% · resets 2h`; R/Y/G via a `band_color(pct)`
   helper (🟢<70 / 🟡70–90 / 🔴>90).
6. **`…/prefs.rs`** — add `usage_refresh_secs: u64` (default 60, clamp ≥30).

---

## APIs

- Reuse `GET /api/usage/claude` → enriched `ClaudeUsageSnapshot` (added `Option`
  fields; backward compatible).
- Upstream (daemon → Anthropic): `GET https://api.anthropic.com/api/oauth/usage`,
  `Authorization: Bearer <token>`. Fallback: local `claude` CLI.

---

## Data Flow

Daemon (on the host, has creds) → `/api/oauth/usage` *or* `claude` CLI fallback,
plus transcript scan + pricing → cache (≥30s) → `/api/usage/claude` →
TUI `api.rs` poll (every `usage_refresh_secs`, ≥30s) → `App` state →
`draw_usage_panel` bottom-left with the color band.

---

## Important Decisions

- **Fetch in the daemon, not the TUI client** — creds live on the host and one
  cache serves TUI + dashboard + PWA; centralizes the remote-IP risk. Chosen
  over per-client fetch because creds + single source. Mirrors the existing
  `usage.rs` / `host_metrics` server-owned pattern.
- **Pull (60s poll) over a broadcast ticker** — usage moves slowly; pull reuses
  the existing dashboard pattern; broadcasting (like `host.metrics` @2s) is
  overkill here.
- **Real `/api/oauth/usage` % over the tokens-vs-configured-tier guess** —
  accurate plan headroom; the old `window_tokens` guess drops to last-resort
  display only.
- **Estimated $ via a static pricing table, labeled "est."** — user wants
  dollars; no billing API exists for subscriptions. Populates the existing
  `cost_usd` plumbing.
- **Additive `Option` fields, not a new type** — older daemons/clients tolerate
  absence (mirrors the pre-v0.6.7 capabilities-absence pattern).

---

## Risks

- **Undocumented endpoint may break** → CLI PTY fallback (required by spec); the
  `source` field surfaces which path produced the number.
- **Remote-VPS IP tripping account rate-limit signals** → fetch only from the
  daemon; respect `HTTPS_PROXY`; server cache keeps cadence low. *Accepted
  residual risk — documented, user approved.*
- **Token leakage** → creds reader returns the token only, never logs it,
  redacts it from error messages.
- **Endpoint self-rate-limit** → server cache ≥30s + client poll min ≥30s.
- **Pricing drift** (model prices change) → single dated static table with a
  source comment; estimate only.
- **Type drift server ↔ CLI** → `Option` fields + JSON-stable names; add a test
  asserting deserialization tolerates missing fields.

---

## Out of scope (YAGNI)

No generic multi-provider usage abstraction (Codex stays as-is; Gemini/OpenCode
are future specs). No usage history/charts. Per-session `cost_usd` population is
optional — only if it falls out of the pricing table cheaply.
