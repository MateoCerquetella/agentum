# Notes — 001 Claude account usage readout

History/overflow for this spec (STATE.md stays lean; detail lives here).

---

## Reviewer sign-off (2026-05-29) — feedback contract

**Verdict: PASS → DONE.** Reviewer trusts the Tester verdict (5/6 AC pass,
CI green 285/0/4; AC6 deferred as a documented live-account manual proof,
human-approved). Reviewer gate: all 6 items pass.

### 1. What worked well

- **Security-first credential handling.** `read_claude_oauth_token` is
  read-only across three sources (env → `~/.claude/.credentials.json` →
  macOS keychain), the token is never logged, and `redact_token` scrubs it
  from every error before it reaches `tracing::debug!`. Directly retires the
  spec's "credential handling" risk.
- **Honest graceful degradation.** `enrich_claude` sets `source="scan"` on no
  token / fetch failure and leaves `limit_pct=None`, so the UI renders
  "plan usage unavailable" instead of a wrong number — exactly the
  amended Fallback AC.
- **Backward compatibility done right.** Additive `Option` fields + a manual
  `Deserialize` with `#[serde(default)]` on both server and the CLI mirror
  (`ClaudeUsage`) — older daemons/clients tolerate absence. Mirrors the
  pre-v0.6.7 capabilities-absence pattern.
- **Risk mitigations are real, not aspirational.** Remote-IP: fetch only in
  the daemon; reqwest reads `HTTPS_PROXY` from the env "for free" (documented).
  Self-rate-limit: `prefs.usage_refresh` floors at `USAGE_REFRESH_MIN_SECS`
  *and* the route caches ≥30s. Pricing drift: a single dated static table.
- **Clear, decision-encoding comments** throughout; thresholds (`band_color`)
  are pure and unit-tested at the exact boundaries (69/70/89/90/91).

### 2. Areas for improvement

- Minor: `estimate_cost_usd` prices the whole token bucket at the *input*
  rate (`model_input_price_per_token`). It is a deliberate simplification for
  a figure already labeled **"est."**, and the spec explicitly accepts a
  notional number on subscription plans — so this is acceptable as shipped,
  noted only as a future-iteration refinement (split input/output/cache
  pricing) if per-session cost accuracy ever matters.

### 3. Risks / debt

- **One documented TODO:** `TODO(spec-001)` — the richer `claude`-CLI PTY
  plan-usage scrape fallback, deliberately deferred (high-risk interactive
  scrape). Recorded in code, `spec.md`, and `tasks.md`. No hidden debt.
- **AC6 (Correctness)** remains a live-account manual proof. By construction
  `limit_pct = max(5h,7d)` reads the identical `/api/oauth/usage` endpoint
  `claude` uses, so it matches by construction; a side-by-side run on a real
  Max/Pro account is the only outstanding confirmation.

### 4. Recommendations

- None blocking. When a second provider readout is added (Codex/Gemini per the
  spec's "Future" note), lift `band_color`/`band_glyph`/`format_resets_in`
  into a shared helper rather than copying.

---

## Release / merge status (discovered 2026-05-29)

The implementation is **already merged to `main` and released**:

- All spec-001 code landed in commit `5caa0a4` (bundled with the hosts-install
  work; the commit subject is hosts-focused but the diff includes usage.rs
  +422, ui.rs +181, app.rs +225, api.rs +75, prefs.rs +46, routes/usage.rs +42).
- `origin/main` = `804578b` = **v0.9.0**, whose CHANGELOG documents "Claude
  account usage tracking" (band colors, refresh, graceful degradation).
- `feat/001-claude-account-usage` == local `main` == `5caa0a4`, which is the
  v0.9.0 base; local `main` fast-forwards cleanly to `origin/main`.

There is nothing left to merge — the SDD cycle's remaining work was the
bookkeeping (Tester + Reviewer gates), now complete.
