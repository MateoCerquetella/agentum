---
phase: 260526-ma9-clipboard-broker
verified: 2026-05-26T00:00:00Z
status: gaps_found
score: 10/12 must-haves verified
overrides_applied: 0
gaps:
  - truth: "Default `agentum clip-agent` connects to every profile in profiles.toml and reconnects with exponential backoff capped at 30s"
    status: failed
    reason: "run_default_loop is an explicit placeholder. After enumerating profiles it calls std::future::pending::<()>().await with a tracing::info log that literally calls itself a placeholder. There is no WS connect, no arboard read, no upload POST, no reconnect logic. The supporting pure-function helpers (profile_ws_url, backoff_for_attempt, classify_arboard_error) exist and are unit-tested but are NEVER called from any code path."
    artifacts:
      - path: "crates/agentum/src/commands/clip_agent.rs"
        issue: "run_default_loop (lines 409-433) is std::future::pending — no WS plumbing, no arboard task, no upload POST. Comments at lines 403-408 and 427-431 acknowledge the placeholder explicitly."
    missing:
      - "Per-profile spawn loop: for each profile resolve bearer token from credentials.toml, build URL with profile_ws_url(base, token), connect via tokio_tungstenite::connect_async_tls_with_config honoring pinned fingerprint + insecure flag (mirror crates/agentum/src/commands/terminal/api.rs::connect_events_ws)."
      - "Per-WS-message handler: on Message::Text parse JSON, branch on type=='clipboard_request', extract request_id + session_id, tokio::task::spawn_blocking(|| arboard::Clipboard::new()?.get_image()), classify errors via classify_arboard_error and send back {\"type\":\"no_image\",\"request_id\":\"…\"} for NoImage/Retry/Fatal."
      - "Upload POST: reqwest::Client::new().post(<base>/api/sessions/{session_id}/uploads).bearer_auth(&token).header(\"X-Clipboard-Request-Id\", request_id.to_string()).body(encode_rgba_as_png(w,h,&rgba)?).send().await."
      - "Reconnect loop with backoff_for_attempt(n) — already tested, just needs a caller. Reset n=0 after a connection sustained ≥30s."
      - "tokio::select! tying ctrl_c + all profile tasks together so SIGINT/SIGTERM drains cleanly."
  - truth: "Fresh `curl … | sh` install on macOS/Linux loads the clip-agent on next login (gated by the existing INTERACTIVE flag); CI / non-interactive installs skip the autostart shellout"
    status: partial
    reason: "install.sh's install_clip_agent_autostart() correctly gates and (when ungated) shells out to `agentum clip-agent --install`. That subcommand correctly writes the plist/unit and bootstraps it with launchctl/systemctl. HOWEVER: when launchd/systemd then start the service on next login, it invokes `agentum clip-agent` with no flags — which hits run_default_loop, logs the placeholder line, and parks on std::future::pending forever. The service is technically loaded and active but cannot ever fulfill a clipboard request. End-user effect: Ctrl-V on Mac→VPS will always 503 with kind=agent_not_connected (or, worse, if the receiver_count check passes because the WS DID get opened in some future code path, will time out)."
    artifacts:
      - path: "crates/agentum/src/commands/clip_agent.rs"
        issue: "Autostart wires the service to a no-op binary."
    missing:
      - "Same as above: fill in run_default_loop. The autostart scaffolding is correct and will pick up the real loop automatically once it lands."
---

# Phase 260526-ma9: Mac→Remote Image Paste via Clipboard Broker — Verification Report

**Phase Goal:** Add seamless Mac→remote image paste via daemon-brokered clipboard agent. Four commits deliver: (1) broker route on the daemon, (2) `agentum clip-agent` CLI subcommand with WS connect-loop + arboard read + upload POST, (3) TUI Ctrl-V broker-first with arboard fallback, (4) install/update autostart via launchd + systemd user units.

**Verified:** 2026-05-26
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

The core user-facing truth — **"Cmd+V on Mac pastes an image into a session on a VPS"** — is **NOT achieved**. Three of the four scaffolding commits landed correctly (broker route, TUI broker-first, install autostart), but the executor explicitly deferred the production WS connect-loop body inside `agentum clip-agent`. The CLI subcommand exists, accepts all the right flags, and the autostart will load the service on next login — but the loaded service immediately parks on `std::future::pending::<()>().await` and never opens a WebSocket. The result: every Ctrl-V from a remote TUI will see `receiver_count == 0` on the daemon and 503 `agent_not_connected` immediately.

The executor flagged this deviation honestly in `260526-ma9-SUMMARY.md` ("Deviations from Plan → Clip-agent default loop body is a placeholder"). This verification confirms the flag and classifies it as a HARD GAP, not `human_needed`.

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `POST /api/clipboard/request` fast-fails (≤50ms) with kind=agent_not_connected when no agent connected | VERIFIED | `routes/clipboard.rs:178-186` early-return on `receiver_count() == 0`. Test `request_503_when_no_agent_connected` passes. |
| 2 | `POST /api/clipboard/request` correlates the matching upload via `X-Clipboard-Request-Id` header | VERIFIED | `routes/uploads.rs:164` reads header. `routes/clipboard.rs::tests_helpers_complete_clipboard_request` is the shared sink. Test `request_succeeds_when_agent_uploads` passes. |
| 3 | WS `/api/clipboard/agent` requires bearer auth via `?token=` and is NOT in the public allow-list | VERIFIED | `grep -v '^[[:space:]]*//' crates/agentum-server/src/auth.rs \| grep -c clipboard` returns 0. WS lives behind `lib.rs::router()`'s `require_token` layer. |
| 4 | Direct uploads (no `X-Clipboard-Request-Id`) work unchanged | VERIFIED | `routes/uploads.rs:164` uses `if let Some(...)` — the header path is purely additive; the 200 response shape and side-effects are identical when absent. (Caveat: no full-handler regression test, but the diff inspection is conclusive.) |
| 5 | Default `agentum clip-agent` connects to every profile and reconnects with exponential backoff capped at 30s | **FAILED** | `clip_agent.rs::run_default_loop` (lines 409-433) is `std::future::pending::<()>().await`. No WS connect, no reconnect. `backoff_for_attempt` is tested but has zero non-test call sites. |
| 6 | PNG encoder lives in `crates/agentum/src/clipboard.rs` and is reused by both clip-agent and TUI fallback | VERIFIED | `crates/agentum/src/clipboard.rs:16` defines `pub fn encode_rgba_as_png`. `terminal/app.rs` imports via `use crate::clipboard::encode_rgba_as_png`. Tests live with the function in `crates/agentum/src/clipboard.rs:30-67`. (clip-agent will reuse it once the loop body lands — currently the function is reachable but unreferenced from the clip-agent code path.) |
| 7 | `agentum clip-agent --install` writes plist/unit idempotently with NO real launchctl/systemctl shellouts in tests | VERIFIED | `clip_agent.rs::install()` (macOS at lines 242-286, Linux at lines 288-315) shells out to launchctl/systemctl only at runtime. Unit tests (`plist_xml_renders_with_user_paths`, `systemd_unit_renders_with_user_paths`) call only the pure render functions. |
| 8 | TUI Ctrl-V tries broker first; falls back to local arboard ONLY on `ClipboardRequestError::AgentNotConnected` | VERIFIED | `terminal/app.rs:3414` `target_client.request_clipboard(id, 3000).await` is the first call. `classify_clipboard_result` (line 3457) returns `FallbackToArboard` exclusively for `AgentNotConnected`. Test `ctrl_v_falls_back_to_arboard_on_agent_not_connected` passes. |
| 9 | `NoImage` and `Timeout` do NOT trigger the arboard fallback (targeted toasts instead) | VERIFIED | `terminal/app.rs:3458-3463` maps `NoImage` and `Timeout` to `CtrlVDecision::ErrorNoFallback`. Tests `ctrl_v_no_image_kind_does_not_fallback` and `ctrl_v_timeout_kind_does_not_fallback` pass. |
| 10 | Fresh `curl … \| sh` install on macOS/Linux loads the clip-agent on next login (gated by INTERACTIVE); CI skips it | **PARTIAL** | `scripts/install.sh:646-694` correctly gates on `AGENTUM_INSTALL_NO_CLIP_AGENT` + `INTERACTIVE` + platform + `AGENTUM_INSTALL_DRY_RUN`. Shellout to `agentum clip-agent --install` writes the plist/unit and bootstraps it. BUT: the service loads a no-op binary (truth #5). Autostart-as-scaffolding is correct; "loads the clip-agent" in the user-meaningful sense fails because the loaded process does nothing. |
| 11 | `agentum update --skip-clip-agent` sets `AGENTUM_INSTALL_NO_CLIP_AGENT=1` in the spawned `sh -s --` installer env | VERIFIED | `commands/update.rs:51-57` injects the env var when the flag is set. `cli.rs:601,609` threads the flag through `dispatch`. install.sh test 1 confirms the env-var gate stops the hook. |
| 12 | OSC52 pre-existing flakiness unchanged (no new flakiness introduced) | VERIFIED | `cargo test -p agentum --lib osc52_tests` shows the same pre-existing failure (`inside_tmux_uses_dcs_passthrough`) noted in the plan as out-of-scope. Neither this test nor `plain_terminal_uses_bare_osc52` was touched by the four commits. |

**Score:** 10/12 verified, 1 FAILED, 1 PARTIAL (counted with failures).

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/agentum-server/src/routes/clipboard.rs` | Broker route: WS for agents + POST /request for TUIs (`pub fn router()`) | VERIFIED | 572 lines. `router()` at line 98 mounts both routes. All 6 handler-level tests green. |
| `crates/agentum/src/clipboard.rs` | Shared PNG encoder (`fn encode_rgba_as_png`) | VERIFIED | 68 lines. `pub fn encode_rgba_as_png` at line 16. Both moved tests green. |
| `crates/agentum/src/commands/clip_agent.rs` | clip-agent subcommand (default loop + install/uninstall/status/logs) | **STUB (default loop)** + VERIFIED (install/uninstall/status/logs) | 513 lines. install/uninstall/status/logs are production-correct. Default loop (`run_default_loop` lines 409-433) is `std::future::pending`. |
| `crates/agentum-core/src/profiles.rs` | `Profiles::load()` convenience wrapper | VERIFIED | `pub fn load()` at line 91. XDG_CONFIG_HOME → $HOME/.config fallback. Test `profiles_load_reads_xdg_config_home` exists. |
| `crates/agentum/src/commands/terminal/app.rs` | Ctrl-V broker-first flow with arboard fallback | VERIFIED | `spawn_ctrl_v_image_paste` at line 3372 calls `request_clipboard` first (line 3414). `spawn_arboard_paste_direct` (line 3475) is the extracted helper. All 4 (actually 6) ctrl_v tests pass. |
| `scripts/install.sh` | Post-install autostart hook (`install_clip_agent_autostart`) | VERIFIED | Function at line 646. Call sites at lines 731 (post_host) and 851 (update branch). Source-only guard at line 798. Shell test `tests/install_clip_agent_autostart.sh` passes 3/3. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|-----|-----|--------|---------|
| `lib.rs::router` | `routes::clipboard::router` | merge inside bearer-auth layer | VERIFIED | Merged at the same level as `events::router()`, `sessions::router()`. `require_token` layers over the entire merge. |
| `auth::is_public` | `/api/clipboard/*` | NEGATIVE assertion | VERIFIED | Zero non-comment matches for "clipboard" in `auth.rs`. |
| `routes/uploads.rs` | `state.clipboard_pending` | `X-Clipboard-Request-Id` header → pop oneshot → `Uploaded { size_bytes: u64 }` | VERIFIED | `uploads.rs:164` header read. Calls `tests_helpers_complete_clipboard_request` with the proper `Uploaded { size_bytes: u64 }`. |
| `terminal/api.rs::Client` | `POST /api/clipboard/request` | `request_clipboard` returning `Result<UploadResponse, ClipboardRequestError>` | VERIFIED | Method at line 544. 503-kind discriminant decoded into typed error at line 580-588. |
| `commands/clip_agent.rs` | `agentum_core::profiles::Profiles::load` | new convenience wrapper | VERIFIED (call exists) / **NOT WIRED** (in default loop) | `Profiles::load()` called from `run_default_loop:410` and `status():353`. Profile list is enumerated, then dropped — no per-profile task spawn. |
| `commands/update.rs` | `scripts/install.sh` | `--skip-clip-agent` → `AGENTUM_INSTALL_NO_CLIP_AGENT=1` in spawned sh env | VERIFIED | `update.rs:51-57`. install.sh test 1 confirms gate fires. |
| `install.sh::install_clip_agent_autostart` | `agentum clip-agent --install` | `INTERACTIVE` + `AGENTUM_INSTALL_NO_CLIP_AGENT` + dry-run gates | VERIFIED (gate logic) / PARTIAL (downstream payload is a no-op service — see truth #10) | Gates fire correctly. Service installs and loads. Loaded service is a no-op. |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `routes::clipboard::request` handler | `body.session_id`, `body.timeout_ms` | TUI POST | YES — full broker round trip implemented | FLOWING |
| `routes::clipboard::run_agent` WS handler | `frame` from `clipboard_request_bus` | broadcast from `request` handler | YES — frame forwarded to socket | FLOWING |
| `routes::uploads::upload` (X-Clipboard-Request-Id path) | `request_id` from header | clip-agent POST | YES — calls into clipboard_pending | FLOWING |
| `terminal/app.rs::spawn_ctrl_v_image_paste` | `result` from `request_clipboard` | broker HTTP | YES — typed error decoded, branch decision wired | FLOWING |
| **`clip_agent.rs::run_default_loop`** | `profiles` (list of names) | `Profiles::load()` | **NO — enumerated and dropped; no WS opens, no clipboard reads, no uploads** | **DISCONNECTED** |

This is the smoking gun. The clip-agent's data-flow stops at "enumerate profile names and log them." Every helper function downstream of that point (`profile_ws_url`, `backoff_for_attempt`, `classify_arboard_error`, `encode_rgba_as_png` from sibling crate) is unreferenced from any non-test call site.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Broker route tests pass | `cargo test -p agentum-server --lib clipboard` | 6 passed, 0 failed | PASS |
| TUI ctrl_v tests pass | `cargo test -p agentum --lib ctrl_v_tests` | 6 passed, 0 failed | PASS |
| clip-agent pure-fn tests pass | `cargo test -p agentum --lib clip_agent` | 6 passed, 0 failed | PASS |
| install.sh autostart hook test | `bash tests/install_clip_agent_autostart.sh` | OK | PASS |
| `/api/clipboard` absent from `is_public` | `grep -v '^[[:space:]]*//' crates/agentum-server/src/auth.rs \| grep -c clipboard` | 0 | PASS |
| End-to-end Mac→VPS paste smoke test | Manual; requires two hosts + the WS loop body to actually exist | Cannot execute — loop is a placeholder | **FAIL (cannot run)** |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `crates/agentum/src/commands/clip_agent.rs` | 403-408 | Self-described placeholder doc comment: *"Stubbed for now — production hook lives behind `--install`, which is the user-visible surface; the loop body is exercised end-to-end via the integration scenario rather than re-implementing the entire WS plumbing here. A future patch can flesh this out…"* | **BLOCKER** | The primary user-facing feature of this phase does not function. Three commits of scaffolding land around a no-op core. |
| `crates/agentum/src/commands/clip_agent.rs` | 427-431 | `tracing::info!(..., "clip-agent default loop placeholder; install -> launchd/systemd to run in production"); std::future::pending::<()>().await; unreachable!()` | **BLOCKER** | Misleading: the log message says "install -> launchd/systemd to run in production" but installing it just causes launchd to keep restarting a process that immediately parks forever. There is no production code path. |

### Anti-pattern stub classification

Per Step 7 stub classification rules: the `std::future::pending::<()>().await` ON ITS OWN would not necessarily be a stub. But here:
- The function is the documented entry point for the user-visible feature.
- The plan's Manual smoke test (`<verification>::Manual smoke`, line 571-575) explicitly depends on this code path doing real work.
- No other code path opens the WS to `/api/clipboard/agent`. There is no parallel real implementation.
- The executor's own SUMMARY ("Deviations from Plan") classifies it as deferred work.

This is the canonical "task completed, goal missed" failure mode the verifier exists to catch.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| `CLIP-BROKER` | 260526-ma9-PLAN.md | Broker route — WS + POST /request | SATISFIED | routes/clipboard.rs production-quality, 6/6 tests green |
| `CLIP-AGENT-CLI` | 260526-ma9-PLAN.md | clip-agent subcommand (loop + install/uninstall/status/logs) | **BLOCKED** | install/uninstall/status/logs done; default loop is a placeholder. The CLI surface exists but the primary CLI function (default loop) is non-operative. |
| `CLIP-TUI-FALLBACK` | 260526-ma9-PLAN.md | TUI Ctrl-V broker-first with arboard fallback | SATISFIED | terminal/app.rs broker-first wiring + classify_clipboard_result + 4 dedicated tests green |
| `CLIP-AUTOSTART` | 260526-ma9-PLAN.md | install.sh + update --skip-clip-agent | PARTIAL | Scaffolding correct; downstream payload (the loaded service) is a no-op. The autostart works as a mechanism — it just autostarts a non-functional binary. |

### Anti-Patterns Found (already covered above)

### Human Verification Required

None — every gap in this report is grep-verifiable from source. No live-host smoke test could change the conclusion: a function whose body is `std::future::pending::<()>().await` cannot, on any host or in any environment, open a WebSocket or read a clipboard.

### Gaps Summary

The phase delivers **three production-quality scaffolding commits** wrapped around **a deliberately empty core**:

1. The daemon's broker route is real and tested (Task 1, commit f28c35e).
2. The TUI's broker-first Ctrl-V flow with arboard fallback is real and tested (Task 3, commit 2a71df4).
3. The installer's autostart hook is real and shell-tested (Task 4, commit 3d6eeaa).
4. **Task 2's `agentum clip-agent` default loop is a `std::future::pending::<()>().await` placeholder.** All five required behaviors (open WS to `/api/clipboard/agent?token=…`, receive `clipboard_request` frames, read the OS clipboard via `arboard` in `spawn_blocking`, POST PNG to `/api/sessions/{id}/uploads` with `X-Clipboard-Request-Id`, reconnect with exponential backoff) are absent. The plan's Manual Smoke verification path (step 3 in `<verification>`) is impossible to execute against this code.

The user-facing goal — "copy image on Mac, hit Ctrl-V in TUI, image lands as upload" — cannot work today. The system will deterministically 503 with `agent_not_connected` because no agent ever opens a WS to the broker.

The good news: every helper the loop needs is already in place (`profile_ws_url`, `backoff_for_attempt`, `classify_arboard_error`, `encode_rgba_as_png` via the sibling crate, `agentum_core::profiles::Profiles::load`, plus `terminal/api.rs::connect_events_ws` as a reference pattern). The remaining work is a single focused ~150-200 line commit that wires these helpers together inside `run_default_loop`.

## Remediation

Replace the body of `crates/agentum/src/commands/clip_agent.rs::run_default_loop` (currently lines 409-433) with the WS connect-loop. The shape:

```rust
async fn run_default_loop(profile: Option<String>) -> Result<()> {
    let profiles = agentum_core::profiles::Profiles::load().context("load profiles")?;
    let entries: Vec<(String, String, Option<String>)> = profiles
        .list()  // returns (name, url, fingerprint?) tuples
        .into_iter()
        .filter(|(name, _, _)| profile.as_deref().map_or(true, |p| p == name))
        .collect();
    if entries.is_empty() { bail!("no matching profiles found"); }

    let mut joins = Vec::new();
    for (name, base_url, fingerprint) in entries {
        let token = match token_for_profile(&name)? {  // lookup in credentials.toml
            Some(t) => t,
            None => { tracing::warn!(profile = %name, "no token; skipping"); continue; }
        };
        joins.push(tokio::spawn(run_profile_loop(name, base_url, token, fingerprint)));
    }
    tokio::select! {
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = futures_util::future::join_all(joins) => Ok(()),
    }
}

async fn run_profile_loop(name: String, base: String, token: String, fingerprint: Option<String>) {
    let mut attempt: u32 = 0;
    loop {
        let url = match profile_ws_url(&base, &token) {
            Ok(u) => u,
            Err(e) => { tracing::error!(?e, profile = %name, "bad URL"); return; }
        };
        let connect_started = std::time::Instant::now();
        match connect_clipboard_ws(&url, fingerprint.as_deref()).await {
            Ok(socket) => {
                // Reset backoff if previous connection sustained ≥30s.
                if connect_started.elapsed() >= Duration::from_secs(30) { attempt = 0; }
                let _ = run_socket(socket, &base, &token).await;
                // Fall through to reconnect.
            }
            Err(e) => tracing::warn!(?e, profile = %name, attempt, "WS connect failed"),
        }
        tokio::time::sleep(backoff_for_attempt(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

async fn run_socket(mut socket: WsStream, base: &str, token: &str) -> Result<()> {
    while let Some(msg) = socket.next().await {
        let Ok(Message::Text(t)) = msg else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
        if v.get("type").and_then(|k| k.as_str()) != Some("clipboard_request") { continue; }
        let request_id = v.get("request_id").and_then(|r| r.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .context("missing/invalid request_id")?;
        let session_id = v.get("session_id").and_then(|r| r.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .context("missing/invalid session_id")?;

        // arboard MUST run on a blocking thread — it blocks on X11/Wayland/Cocoa IPC.
        let image = tokio::task::spawn_blocking(|| {
            arboard::Clipboard::new()?.get_image()
        }).await?;

        match image {
            Ok(img) => {
                let png = agentum::clipboard::encode_rgba_as_png(
                    img.width as u32, img.height as u32, img.bytes.as_ref()
                ).map_err(|e| anyhow!("encode: {e}"))?;
                let upload_url = format!("{}/api/sessions/{}/uploads", base.trim_end_matches('/'), session_id);
                let resp = reqwest::Client::new()
                    .post(&upload_url)
                    .bearer_auth(token)
                    .header("X-Clipboard-Request-Id", request_id.to_string())
                    .header("Content-Type", "image/png")
                    .body(png)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    tracing::warn!(status = %resp.status(), "upload failed");
                }
            }
            Err(e) => {
                let action = classify_arboard_error(&e);
                tracing::info!(?action, ?e, "arboard read failed; sending no_image");
                // For NoImage/Retry/Fatal: tell the broker no_image so it doesn't wait.
                socket.send(Message::Text(json!({
                    "type":"no_image","request_id": request_id.to_string()
                }).to_string())).await?;
            }
        }
    }
    Ok(())
}
```

Key implementation notes:
- **TLS plumbing**: mirror `crates/agentum/src/commands/terminal/api.rs::connect_events_ws` for the rustls config + fingerprint pin + insecure-mode handling. Don't reinvent it.
- **Token lookup**: `credentials.toml` is the source of truth (NOT `profiles.toml`). The TUI helper that does this is in `crates/agentum/src/commands/terminal/profiles.rs` or sibling — extract it to a pub helper rather than duplicating.
- **arboard MUST go through `spawn_blocking`**: the X11/Wayland/Cocoa pump blocks the runtime if called from an async context.
- **Don't kill the agent on errors**: every per-message failure should send `no_image` back so the broker resolves the request, then continue the loop. Only un-recoverable errors (token rejected, URL malformed) should drop the per-profile task.
- **Backoff reset**: only reset `attempt = 0` after a connection sustained ≥30s. This prevents tight-loop reconnect storms when the daemon is repeatedly closing the WS on connect.

Then re-run the full gate from the plan's `<verification>` block. The Manual Smoke test should now succeed: copy an image on the Mac running clip-agent, attach to the VPS daemon via `agentum terminal --profile vps`, hit Ctrl-V, and within ~1-2s see the upload path appear in the pane.

### Recommended next plan

`/gsd plan-phase --gaps 260526-ma9` to generate a focused follow-up plan that targets exclusively `run_default_loop` (and an integration test that pins the round trip). The four scaffolding commits don't need to be touched.

---

*Verified: 2026-05-26*
*Verifier: Claude (gsd-verifier)*
