# Tasks — 001 Claude account usage readout

- [x] Add Claude OAuth token reader (env `CLAUDE_CODE_OAUTH_TOKEN` →
      `~/.claude/.credentials.json` `claudeAiOauth.accessToken` → macOS
      keychain); never logs the token — `usage.rs::read_claude_oauth_token`
      *(in-module, not a separate `claude_oauth.rs` — documented deviation)*
- [x] Fetch + parse `GET /api/oauth/usage` → `five_hour.utilization`,
      `seven_day.utilization`, `resets_at` — `usage.rs::fetch_oauth_usage`
- [x] Static pricing table + `est_cost_usd` from scanned tokens —
      `usage.rs::estimate_cost_usd` *(in-module, not `pricing.rs`)*
- [x] Extend `ClaudeUsageSnapshot` with additive `Option` fields:
      `limit_pct = max(5h,7d)`, `five_hour_pct`, `seven_day_pct`,
      `resets_at_ms`, `est_cost_usd`, `source`
- [ ] ~~PTY fallback scraping `claude` "Show plan usage limits"~~ →
      **DEVIATION (needs sign-off):** implemented as **graceful
      degradation** instead — on OAuth failure / no token the snapshot sets
      `source="scan"`, `limit_pct=None`, and the UI shows "plan usage
      unavailable" (no wrong number). PTY scrape left as a `TODO(spec-001)`.
- [x] Server-side ≥30s cache in front of `/api/usage/claude`
      (`routes/usage.rs` `OnceLock<Mutex<…>>`, lock held across the fetch to
      dedupe concurrent misses)
- [x] CLI `api.rs`: `claude_usage()` → GET `/api/usage/claude` into the
      tolerant `ClaudeUsage` mirror struct (404 → clean error)
- [x] `prefs.rs`: `usage_refresh_secs` (default 60) + `usage_refresh()` clamp
      ≥30; tick-driven poll in `app.rs` (mpsc + `usage_inflight` coalescing)
- [x] `ui.rs::draw_usage_panel`: header line `est $X · Nk tok · <band> NN% ·
      resets …` + `band_color`/`band_glyph` (🟢<70 / 🟡70–90 / 🔴>90)
- [x] Graceful "unavailable" state when no token / OAuth fails (no wrong
      number, no crash)
- [x] Tests: `max(5h,7d)` + clamp, band thresholds (69/70/89/90/91), snapshot
      deserializes with missing fields, pricing calc, token extraction, token
      redaction, prefs default/clamp
- [~] Manual proof: panel % matches `claude` "Show plan usage limits" for the
      same account at the same time — **DEFERRED (documented):** correctness
      verdict needs a live Max/Pro account + side-by-side run. By construction
      `limit_pct = max(5h,7d)` reads the *identical* `/api/oauth/usage` endpoint
      `claude` itself uses, so the value matches by construction. Tester gate
      approved by human (2026-05-29) with this as a documented manual item.

> **Tester verdict (2026-05-29):** 5/6 ACs PASS with evidence; AC6 (Correctness)
> deferred as the documented live-account manual proof. CI re-verified green:
> `cargo fmt --all -- --check` clean · `clippy -p agentum-server -p agentum-cli
> -p agentum-core --lib -D warnings` clean · `cargo test` (same 3 pkgs) = **285
> passed, 0 failed, 4 ignored** (the 4 ignores are unrelated live-tmux board
> tests). Spec-001 AC tests all present and passing: `band_color_thresholds`,
> `band_glyph_thresholds`, `limit_pct_takes_max_of_windows`,
> `limit_pct_clamps_out_of_range`, `usage_refresh_defaults_and_clamps`,
> `usage_refresh_missing_key_uses_default`, `estimate_cost_prices_known_models_
> and_skips_unknown`, `snapshot_deserializes_with_missing_spec001_fields`,
> `snapshot_round_trips_with_spec001_fields`, `redact_token_scrubs_secret`,
> `scan_claude_returns_sane_shape`.
>
> Each task maps to the spec's acceptance criteria (Content / Band / Refresh /
> Fallback / Graceful failure / Correctness).
