# Codebase Concerns

**Analysis Date:** 2026-05-20

## Tech Debt

**Monolithic TUI app module:**
- Issue: `crates/agentum/src/commands/terminal/app.rs` is 8290 lines — a single file owns input handling, state machine, run-loop, overlays, picker logic, and dozens of unit tests. Any structural change ripples through the whole file.
- Files: `crates/agentum/src/commands/terminal/app.rs`, `crates/agentum/src/commands/terminal/ui.rs` (3059 lines), `crates/agentum/src/commands/terminal/mod.rs` (1192 lines)
- Impact: Long compile times for incremental TUI tweaks; high cognitive load to find the right `app.rs` section; tests stay co-located with implementation but the file is too large for an LLM to load in a single pass.
- Fix approach: Extract per-overlay state machines (Profiles, NewSession, ServerSwitcher, ImagePaste) into sibling files; keep `app.rs` as the dispatch shell. The pattern is already used for `palette.rs`, `theme.rs`, `extensions.rs`.

**Embed-handler routing chain has multiple SPA fallbacks that mask real 404s:**
- Issue: `embed::static_handler` always falls back to `index.html` when an asset isn't found, so requests for stale `/_app/immutable/<old-hash>/foo.js` after a rebuild return the HTML shell with a 200 instead of 404, which the browser then parses as JS and silently fails.
- Files: `crates/agentum-server/src/embed.rs:46-56`
- Impact: A stale dashboard tab against a newer daemon shows blank-screen / broken-import errors that are hard to diagnose.
- Fix approach: Only fall back to `index.html` for routes the SPA owns (no extension, or HTML accept header). Return 404 for `*.js`, `*.css`, `*.map`, `*.webmanifest`.

**Test-only state-construction boilerplate duplicated across routes:**
- Issue: `board.rs`, `board_rules.rs`, and likely others each rebuild a full `AppState` (with `Arc<Mutex<HashMap>>`, ratelimiter, etc.) inline for handler tests.
- Files: `crates/agentum-server/src/routes/board.rs:419-430`, `crates/agentum-server/src/routes/board_rules.rs:132-145`
- Impact: Adding a new field to `AppState` means touching every handler test. Just happened with `stream_positions` (`Arc<std::sync::Mutex<HashMap<Uuid, StreamCheckpoint>>>`).
- Fix approach: Extract a `test_support::mk_state(store)` helper in `agentum-server`, gated behind `#[cfg(test)]`.

**Hardcoded default-columns scattered across crates:**
- Issue: `["todo", "doing", "done"]` is declared at least four times, with the new slice-2 rules feature adding a fifth call site.
- Files: `crates/agentum-server/src/routes/board.rs:50`, `crates/agentum-server/src/rules.rs:18`, `crates/agentum-store/src/lib.rs:312` (default status), `dashboard/src/lib/stores/fleet-board.ts:40`, `dashboard/src/lib/components/BoardItemDialog.svelte:590`
- Impact: Adding a default column (or renaming `doing` → `wip`) requires synchronized edits across the workspace and dashboard.
- Fix approach: Move the constant into `agentum-core` and re-export. The TS side stays a manual mirror until a codegen step lands.

**Legacy localStorage migration code lingers in dashboard:**
- Issue: `dashboard/src/lib/profiles.ts` still reads `agentum_profiles` and `agentum_token` legacy keys, mirror-writes the legacy single-token slot on every active-profile change, and the `LEGACY_MIGRATED_KEY` gate ensures migration runs only once but the cleanup code never deletes the old keys.
- Files: `dashboard/src/lib/profiles.ts:60-68, 183-260, 402-460`
- Impact: localStorage keeps growing with stale data; future profile-storage refactors must keep all three storage shapes intact forever.
- Fix approach: After a release window (≥2 minors), drop legacy keys, remove the migration code path entirely, document the floor version in the CHANGELOG.

**`patch_session` swallows model patches silently:**
- Issue: `patch_session` accepts `model: Option<Option<String>>` from the API but the implementation explicitly drops it with `let _ = model;` and the comment `// Future: patch model — not yet implemented in store`.
- Files: `crates/agentum-server/src/routes/sessions.rs:190-194`
- Impact: A dashboard PATCH with `model` reports success but the value is never persisted. No 400, no warning.
- Fix approach: Either implement `patch_session_model` in the store, or 400 with `model patching not yet supported` until the store function lands.

## Known Bugs

**Auth WS query-string token can leak into shell history / process listings:**
- Symptoms: Bearer token visible in `ps` output of any user on a multi-user box, and potentially in nginx / cloudflare access logs upstream when agentum is behind a proxy.
- Files: `crates/agentum-server/src/auth.rs:96-104` (extracts `?token=`), `dashboard/src/lib/profiles.ts:654` (constructs URL)
- Trigger: Any WebSocket connection from the dashboard or TUI — `/api/events`, `/api/sessions/{id}/stream`.
- Workaround: The daemon's own logs scrub `token=` to `REDACTED` (`crates/agentum-server/src/logging.rs:38-57`), but that doesn't cover external proxies, reverse-proxy access logs, or the browser's `Performance` panel / DevTools history. WS protocol headers would solve this (Sec-WebSocket-Protocol carrying the token) but require server-side parsing.

**Static fallback returns SPA HTML for asset requests:**
- Symptoms: Stale dashboard tabs after a daemon upgrade show a blank page with "unexpected token <" errors in the console.
- Files: `crates/agentum-server/src/embed.rs:46-56`
- Trigger: Reload of a long-open dashboard tab after `npm run build` + `cargo build --release` + daemon restart.
- Workaround: Hard reload / clear cache. The fix is to scope SPA fallback to HTML-content routes (see Tech Debt).

**Workdir picker `/api/fs/list` exposes the entire filesystem to any authenticated client:**
- Symptoms: Any logged-in dashboard user can list directory contents under the daemon's UID, including `~/.ssh`, `~/.aws`, `~/.config/agentum/profiles.toml`, by passing `?path=...`.
- Files: `crates/agentum-server/src/routes/fs.rs:51-110`
- Trigger: `GET /api/fs/list?path=/home/<user>/.ssh&show_hidden=true`.
- Workaround: The endpoint only returns directory entries (not file contents), and is auth-gated, but multi-user scenarios (a shared agentum at the team level) get full path enumeration. Document the single-tenant assumption explicitly, or restrict to a `--workdir-root` allowlist.

**`patch_session` reports success while dropping model updates:**
- Symptoms: PATCH `/api/sessions/{id}` with `{"model": "..."}` returns 200, but a subsequent GET shows the old model.
- Files: `crates/agentum-server/src/routes/sessions.rs:190-194`
- Trigger: Any client tries to change a session's model.
- Workaround: None on the client side. Server should 400 until the store call exists.

**Watchdog `IDLE_AFTER_QUIET = 3s` window can fire premature `agent.finished`:**
- Symptoms: For non-Claude adapters (codex, gemini, cursor) without a `busy_signature`, the watchdog declares the session idle after 3 s of unchanged pane content — even though many ratatui-based agents render no visible activity during long tool calls.
- Files: `crates/agentum-watchdog/src/lib.rs:44`
- Trigger: An agent runs a multi-second tool (search, large file read) and emits no terminal output during it.
- Workaround: Notification toasts may fire spuriously. Real fix is to declare a `busy_signature` per adapter (already done for Claude; others are pending).

## Security Considerations

**Bearer token in WS query string (`?token=`):**
- Risk: Tokens land in `ps` output, reverse-proxy access logs, browser history, error reporters.
- Files: `crates/agentum-server/src/auth.rs:96-104`, `crates/agentum-server/src/logging.rs:38-57`, `dashboard/src/lib/profiles.ts:654`
- Current mitigation: Daemon's `tracing` access log is scrubbed; the cert is self-signed by default so the token never traverses the public internet in clear text when the operator stays on TLS.
- Recommendations: Move to `Sec-WebSocket-Protocol: bearer.<token>` (browser-accessible WS subprotocol), keep `?token=` as a fallback for older clients during a transition window.

**Self-signed TLS + HSTS deliberately omitted:**
- Risk: First-connect MITM is the only window before fingerprint pinning; subsequent connections trust the cached fingerprint, but new devices have no out-of-band channel.
- Files: `crates/agentum-server/src/tls.rs:30-65` (fingerprint export), `crates/agentum-server/src/headers.rs:14-17, 67-72`
- Current mitigation: Documented TOFU bootstrap; `/api/cert/fingerprint` is publicly readable so a second device can verify; the operator is expected to read the fingerprint off the host TTY at first contact.
- Recommendations: Add a `--public-host <name>` mode that integrates with letsencrypt + auto-renewal; document the recommended posture for deploying behind a real reverse proxy with HSTS upstream.

**`--no-auth` flag bypasses everything:**
- Risk: Combined with `AGENTUM_EXPOSE=1` (LAN bind), all routes become reachable without credentials.
- Files: `crates/agentum-server/src/auth.rs:150-180`, `crates/agentum/src/commands/serve.rs:23-26`
- Current mitigation: tracing logs a `WARN` at boot when `--no-auth` is set; CLI help calls out "do NOT expose this to untrusted networks".
- Recommendations: When `--no-auth` is combined with a non-loopback bind address, fail at boot unless the user passes a second `--i-know-what-im-doing` flag.

**`/api/fs/list` is a directory-enumeration oracle:**
- Risk: Any authenticated user can enumerate filesystem layout under the daemon's UID. Useful first step for an attacker who got a token via other means.
- Files: `crates/agentum-server/src/routes/fs.rs`
- Current mitigation: Auth-gated; returns directories only, never file contents.
- Recommendations: Restrict to a configured workdir root (`AGENTUM_WORKDIR_ROOT` or `--workdir-root /path`), reject paths outside it.

**Bearer tokens stored in dashboard `localStorage`:**
- Risk: XSS reads the token; first-class scripts (the CSP allows `'unsafe-inline'` for SvelteKit's bootstrap) could in principle exfiltrate it.
- Files: `dashboard/src/lib/profiles.ts:60-68, 123-129`, `crates/agentum-server/src/headers.rs:38-50` (CSP)
- Current mitigation: CSP restricts `script-src` to `'self'` + inline (no third-party CDNs); `connect-src` allows any HTTP/WS because of multi-endpoint profiles. No third-party scripts are loaded in the production build.
- Recommendations: Move tokens to httpOnly cookies for browser sessions (would require CSRF protection); keep localStorage for the multi-endpoint case where cookies don't work across origins.

**Cert-server on a separate plaintext port serves the PEM unauthenticated:**
- Risk: Anyone on the LAN can grab the cert fingerprint without auth. This is intentional (TOFU bootstrap), but it's worth surfacing.
- Files: `crates/agentum-server/src/lib.rs:303-321` (`cert_server_router`)
- Current mitigation: Only the public cert PEM is served (not the private key), and the fingerprint is meant to be public.
- Recommendations: Document the threat model; rate-limit the cert endpoint to avoid scan amplification.

**Argon2 cost is the library default:**
- Risk: The default Argon2id params are reasonable today but become inadequate as hardware improves.
- Files: `crates/agentum-server/src/auth.rs:52-59`
- Current mitigation: Argon2 spawned on the blocking pool so concurrent logins don't stall the async runtime.
- Recommendations: Pin explicit `Params::new(...)` (memory cost, iterations) and document a rotation strategy.

## Performance Bottlenecks

**Watchdog samples panes every 1 s per session — `tmux capture-pane` × N:**
- Problem: Each session runs `tmux capture-pane -p -S -100` + `tmux capture-pane -p -S 0` + `tmux display-message -p #{pane_current_command}` once per second.
- Files: `crates/agentum-watchdog/src/lib.rs:35, 233-264, 297-326`
- Cause: Per-session polling with no debouncing or backoff.
- Improvement path: For idle sessions, slow the cadence to 5 s after N consecutive no-change ticks; reset to 1 s on first change. Or replace polling with `tmux pipe-pane` activity hooks for the busy/idle classifier.

**Pane log files grow unbounded:**
- Problem: `tmux pipe-pane` appends to `<XDG_STATE>/agentum/<id>.log` for the lifetime of the daemon. No rotation.
- Files: `crates/agentum-tmux/src/lib.rs:226-243` (`pipe_pane`), `crates/agentum-store/src/paths.rs` (`pane_log`)
- Cause: Append-only logs are the source of truth for the WS resume / scrollback path.
- Improvement path: Rotate at N MB with the resume checkpoint snapped to the new file's start; or trim the head once `stream_positions[id]` advances past a threshold.

**Embedded bundle bloats the binary:**
- Problem: `rust-embed` ships every file under `dashboard/build/` baked into the binary, currently ~844 KB on disk and embedded twice (once compressed, once decompressed at runtime).
- Files: `crates/agentum-server/src/embed.rs`, `crates/agentum-server/Cargo.toml:14` (`rust-embed = { version = "8", features = ["mime-guess"] }`)
- Cause: Compile-time embedding gives single-binary distribution, but every `cargo build` re-reads the bundle.
- Improvement path: Enable `compression` feature on `rust-embed`; or split into a `--dashboard-dir` runtime flag for development and keep embedding only for release builds.

**Forgotten dashboard rebuild serves OLD bundle silently:**
- Problem: `dashboard/src/` change → `npm run build` produces `dashboard/build/` → but `cargo build` is required to bake the new bundle into the daemon binary. Skipping the second step ships the old SPA against the new backend.
- Files: `crates/agentum-server/src/embed.rs:19-21`, `crates/agentum-server/build.rs` (cargo:rerun-if-changed)
- Cause: `rust-embed` is compile-time only; the running daemon has no hot-reload path. CLAUDE.md documents the workflow explicitly.
- Improvement path: A debug-mode `static_handler` that reads from `dashboard/build/` at runtime when an env var is set, so frontend dev doesn't require `cargo build` per save.

**SQLite pool capped at 8 connections shared across watchdog + server:**
- Problem: 8 concurrent writes is small for a daemon with N watchdog tasks + M websocket clients + interactive HTTP traffic.
- Files: `crates/agentum-store/src/lib.rs:68-70`
- Cause: Default-ish setting; works fine for solo-developer use.
- Improvement path: Lift to 16-32 with WAL-friendly tuning; benchmark under a fleet of 20+ active sessions.

**`spawn_blocking` for sync work on the host metrics path:**
- Problem: Host metrics use `spawn_blocking` for sysinfo calls which is fine, but the metric ticker runs on every connected client (the broadcast pattern), so the underlying sample is shared. Verify no duplication.
- Files: `crates/agentum-server/src/routes/host.rs:100`
- Cause: Intentional — sysinfo polls block the runtime otherwise.
- Improvement path: Already correct.

## Fragile Areas

**Watchdog regex / signature strings:**
- Files: `crates/agentum-watchdog/src/lib.rs:136-142` (`Context low.*<\s*50%`), `crates/agentum-executor/src/adapters.rs` (per-adapter `crash_signatures`, `busy_signature`, `awaiting_input_signatures`)
- Why fragile: Tied to substrings the upstream agent CLIs print. A Claude CLI update that rewords "esc to interrupt" or "Context low" silently breaks busy/idle detection. v0.7.47/v0.7.50/v0.7.52 fixes were all watchdog signature churn.
- Safe modification: Add a regression test that pipes a known transcript through `classify_activity`; bump signatures behind an env var to keep the old one as fallback during a transition window.
- Test coverage: `crates/agentum-watchdog/src/lib.rs:594+` has a `context_low_regex` test, but per-adapter busy/awaiting signatures are tested only indirectly.

**tmux IPC fragility:**
- Files: `crates/agentum-tmux/src/lib.rs`
- Why fragile: Every call shells out to `tmux`, parsing string output. `tmux has-session` exit codes, `capture-pane -p` newline conventions, `display-message -p '#{pane_current_command}'` field names — all coupled to the tmux binary version. The recent fix to use `-S 0` viewport-only captures (v0.7.50) was a tmux behavior pin.
- Safe modification: Continue using long-form arg names; never rely on positional output ordering; document the minimum tmux version (≥3.0 per `resize_window` comment) in `agentum doctor`.
- Test coverage: `lifecycle_smoke` integration test in `crates/agentum-tmux/src/lib.rs:350-369` is the only end-to-end check.

**Claude session UUID pinning:**
- Files: `crates/agentum-executor/src/adapters.rs:32-37, 48-73` (`ClaudeAdapter::launch`), `crates/agentum-server/src/transcript_store.rs:14-25`
- Why fragile: `--session-id` vs `--resume` toggle inside `claude_transcript_exists` depends on the file landing at the deterministic path `transcript::transcript_path_for(workdir, agentum_session_id)`. If Claude Code changes its transcript layout, both the adapter and the watcher break together.
- Safe modification: Treat the transcript path resolver as a single source of truth; add a smoke test that asserts the path matches `~/.claude/projects/<enc>/<uuid>.jsonl`.

**`StreamCheckpoint` in-memory only:**
- Files: `crates/agentum-server/src/lib.rs:94-95` (`stream_positions`), `crates/agentum-server/src/routes/sessions.rs:617-640`
- Why fragile: Daemon restart wipes all resume checkpoints. The handler falls through to a fresh `capture-pane` snapshot when no checkpoint exists, but that snapshot clobbers the client's preserved parser state. The v0.6.26 fix already covered the missing-checkpoint case; a daemon restart during an active session still has visible jank.
- Safe modification: Persist checkpoints to SQLite on disconnect (one row per session, last-write-wins) so a daemon restart can still serve a delta replay.

**Auth ratelimiter is per-process, in-memory:**
- Files: `crates/agentum-server/src/ratelimit.rs`
- Why fragile: A daemon restart clears all rate-limit windows. Memory is bounded by an opportunistic 1024-key sweep, so a flood of distinct attacker IPs evicts legitimate entries.
- Safe modification: Move to a sqlite-backed implementation, or accept the loss and document.

**Per-adapter YOLO marker translation:**
- Files: `crates/agentum-executor/src/lib.rs:111-131, 85-87`, `crates/agentum-executor/src/adapters.rs` (per-adapter `yolo_flag()`)
- Why fragile: Two adapters (`opencode`, `aider`) currently return `None` for `yolo_flag()` because the right spelling hasn't been verified. If a user enables YOLO for those tools the marker is silently dropped — not an error, but the safety-bypass switch is a no-op. CLAUDE.md flags this as a known gotcha.
- Safe modification: Either verify and pin the flag spellings, or surface an `unsupported_yolo` warning event on session start when YOLO is on but `yolo_flag()` is `None`.

**`bottom_lines` viewport classification:**
- Files: `crates/agentum-watchdog/src/lib.rs:356-371, 504-540`
- Why fragile: Trims the last 20 lines of `capture-pane -S 0` output to detect busy/idle. Agents that anchor their UI higher than 20 lines (or scroll the spinner off-screen during a long tool call) fool the classifier.
- Safe modification: Inspect the viewport top-to-bottom for the spinner; fall back to "is the foreground process emitting bytes" via pane log activity.

## Scaling Limits

**One SQLite database per daemon:**
- Current capacity: A typical instance with 10-50 sessions, 100-1000 board items, 10K events. WAL-mode SQLite handles this comfortably.
- Limit: Multi-tenant team usage with thousands of sessions and aggressive event production (every watchdog tick can emit). The event table grows monotonically.
- Scaling path: Periodic event-table compaction (delete rows older than N days); switch to a server-grade DB (postgres) behind the `Store` trait once that becomes a real bottleneck.

**Broadcast bus capacity = 1024 events:**
- Current capacity: 1024 events buffered before slow consumers see `Lagged`.
- Limit: A burst of state changes (e.g. bulk-import 200 sessions, each emitting 5 events) can overflow.
- Scaling path: Increase capacity for known-burst workflows; add a "live-tail vs snapshot" mode where consumers that lag fall back to a re-sync via REST.

**Per-IP rate limit cache evicts at 1024 keys:**
- Current capacity: 1024 distinct IPs concurrently rate-limited.
- Limit: A wide IPv4 / IPv6 scan rotates through far more than 1024 source addresses.
- Scaling path: Move to a TTL-backed LRU; or accept this as best-effort defense and rely on upstream firewalls.

**Watchdog task fan-out:**
- Current capacity: One tokio task per `Running` session, each waking on a 1 s `interval`.
- Limit: At 100+ running sessions the tmux subprocess overhead dominates.
- Scaling path: Single shared task that batches `tmux capture-pane` calls for all sessions in one tick.

## Dependencies at Risk

**`rust-embed` compile-time coupling:**
- Risk: Forgetting `cargo build` after `npm run build` is the #1 footgun (CLAUDE.md "Common gotchas"). The `build.rs` stub-creator hides the failure even further by writing a placeholder index.html so the binary still compiles.
- Files: `crates/agentum-server/build.rs`, `crates/agentum-server/Cargo.toml:14`
- Impact: Production daemons can ship a stale dashboard if release tooling forgets the order.
- Migration plan: Add a release-build assertion that `dashboard/build/index.html` is newer than `dashboard/src/`'s most-recent mtime; bail with a clear error if not.

**`crossterm` 0.28 OSC handling:**
- Risk: From `~/.claude/.../memory/MEMORY.md`: crossterm 0.28 errors on OSC sequences; OSC 52 reads require a /dev/tty bypass or daemon HTTP route.
- Files: `crates/agentum/src/commands/terminal/` (TUI layer)
- Impact: OSC 52 clipboard reads silently fail.
- Migration plan: Track crossterm upstream OSC support; for now keep the documented bypass.

**`vt100` 0.15 scrollback opacity:**
- Risk: From `crates/agentum/src/commands/terminal/term.rs:93` — vt100 0.15 doesn't expose the precise scrollback depth.
- Impact: Scrollback navigation in the embedded terminal panel relies on heuristics.
- Migration plan: Watch vt100 changelog for a `scrollback_lines()` accessor; switch crates if a better-instrumented alternative emerges.

**`sqlx::migrate!` at startup:**
- Risk: Every daemon start runs pending migrations. A faulty migration (or one that times out on a large board table) blocks boot. Migration `0014` is the most recent and shipped in this work-in-progress.
- Files: `crates/agentum-store/src/lib.rs:73`, `crates/agentum-store/migrations/0014_board_column_rules.sql` (untracked, WIP)
- Migration plan: Snapshot the DB before applying migrations; add a `--skip-migrations` flag for recovery.

## Missing Critical Features

**No structured backup / export path:**
- Problem: Operators have no first-class way to back up a daemon's SQLite + transcripts + TLS state.
- Blocks: Trivial migration between hosts; disaster recovery; cross-machine session import.

**No multi-user roles / ACLs:**
- Problem: Every authenticated user has full admin access. Board rules editing, session start/stop, fs listing are all single-tier.
- Blocks: Team deployments where some users should only read, or only act on their own sessions.

**Pre-v0.6.7 capability negotiation is one-way:**
- Problem: Clients gate on `capabilities` from `/api/health`. New server-side capabilities show up there, but there's no way for the *client* to advertise its own version to the server so the server can adapt response shape.
- Files: `crates/agentum-server/src/routes/health.rs:18`, `crates/agentum/src/commands/terminal/api.rs:331-348`
- Blocks: Server-side optimisations that depend on client capability (e.g. only send delta events when the client supports replay).

**No transcript support for non-Claude tools:**
- Problem: Plan/Todos/Tasks panel only works for Claude sessions. Codex/Gemini/Cursor/Hermes show "No plan yet" forever.
- Files: `crates/agentum-server/src/transcript_store.rs:21-25`
- Blocks: Feature parity in the right rail for non-Claude agents.

## Test Coverage Gaps

**Watchdog signature classification:**
- What's not tested: Per-adapter `busy_signature` / `awaiting_input_signatures` matching against realistic pane snapshots.
- Files: `crates/agentum-watchdog/src/lib.rs:365-371` (classify_activity call site), `crates/agentum-executor/src/adapters.rs` (per-adapter signatures)
- Risk: A vendor renames their spinner footer; watchdog stops detecting busy/idle; sidebar dots stay wrong for an entire release.
- Priority: High.

**Multi-session resume / WS reconnect under daemon restart:**
- What's not tested: Restart the daemon while a session has active stream consumers — does the resume path degrade gracefully?
- Files: `crates/agentum-server/src/routes/sessions.rs:458-849`
- Risk: Visible terminal corruption on resume; the v0.6.26 fix path is well-commented but not covered by an integration test.
- Priority: High.

**Auth middleware behaviour with `--no-auth`:**
- What's not tested: That every public/private route is exercised in both modes; that `extract_token` returns `None` doesn't accidentally pass.
- Files: `crates/agentum-server/src/auth.rs:74-104`
- Risk: A future refactor of `is_public` or `extract_token` opens a hole.
- Priority: High.

**`/api/fs/list` boundary cases:**
- What's not tested: Symlink loops, `..` traversal, very deep paths, encoding edge cases.
- Files: `crates/agentum-server/src/routes/fs.rs`
- Risk: Path traversal / symlink-walked file listing surprises.
- Priority: Medium (the route is auth-gated, but it's still an enumeration oracle).

**TUI overlay dispatch under `RunOutcome::SwitchProfile`:**
- What's not tested: The Ctrl-S profile switch soft-restart path, especially around socket teardown + reconnect.
- Files: `crates/agentum/src/commands/terminal/app.rs` (RunOutcome), `crates/agentum/src/commands/terminal/mod.rs:65+`
- Risk: A future change to `connect_once` leaks resources or reuses stale state across switches.
- Priority: Medium.

**Per-tool YOLO marker translation:**
- What's not tested: That an adapter with `yolo_flag() == None` actually drops the marker (vs forwarding it raw and crashing the binary).
- Files: `crates/agentum-executor/src/adapters.rs`, `crates/agentum-executor/src/lib.rs:120-131`
- Risk: v0.6.23 already shipped a codex crash from this. Adding a passthrough tool without thinking about YOLO could regress.
- Priority: Medium (one assertion per adapter test would close this).

**Board rules merge layer:**
- What's not tested: That DB overrides correctly shadow the const matrix, including the edge case where an override sets `required_fields: []` (drop the gate entirely).
- Files: `crates/agentum-server/src/rules.rs:36-52`, `crates/agentum-server/src/routes/board_rules.rs` (WIP)
- Risk: The spec calls out `[]` as the "drop the gate" signal; a missing test could let a future refactor invert it.
- Priority: Medium (covered by handler tests in `board_rules.rs` per the WIP work, but not yet committed).

---

*Concerns audit: 2026-05-20*
