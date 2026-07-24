//! Session pane env + MCP/endpoint provisioning: the loopback env every local
//! pane launches with (`pane_env`), endpoint-drift detection + re-provisioning
//! for sessions whose embedded-server URL/token moved across a restart
//! (`endpoint_drifted` / `reprovision_env` / `reprovision_session`). `use
//! super::*` pulls the parent route module's imports; the helpers the `create`/
//! `start` handlers call are `pub(super)`.

use super::*;

/// The loopback env every LOCAL pane is launched with. `AGENTUM_API_URL` lets an
/// `agentum` CLI run inside the pane find THIS server (the embedded desktop server
/// binds an ephemeral port, so a hardcoded guess would miss it); the hook URL/token
/// let the agent curl lifecycle events back. `api_base` is the embedded server's own
/// URL when known, else the standalone daemon's conventional `127.0.0.1:8822`. The
/// hook URL is DERIVED from the same base — never a separate hardcoded port, which
/// previously pointed every hook at 8822 regardless of where the server actually was.
pub(super) fn pane_env(
    api_base: Option<&str>,
    session_id: Uuid,
    session_name: &str,
    hook_token: &str,
) -> Vec<(String, String)> {
    let base = api_base.unwrap_or("http://127.0.0.1:8822");
    vec![
        ("AGENTUM_API_URL".to_string(), base.to_string()),
        // The orchestration handle for an agent running in this pane: its session
        // name. `agentum orchestration send/check` default `--from`/`--terminal`
        // to this, so an agent can mail siblings without knowing its own name.
        (
            "AGENTUM_TERMINAL_HANDLE".to_string(),
            session_name.to_string(),
        ),
        (
            "AGENTUM_HOOK_URL".to_string(),
            format!("{base}/api/sessions/{session_id}/hook"),
        ),
        ("AGENTUM_HOOK_TOKEN".to_string(), hook_token.to_string()),
    ]
}

/// Percent-encode a worktree path for use as a `?worktree=` query value, without
/// pulling in a URL-encoding dependency. Keeps the URL-unreserved set (plus `/`,
/// which is query-safe and keeps the path readable); everything else becomes
/// `%XX`. axum decodes it back via `serde_urlencoded` on the `/mcp` handler.
fn encode_worktree_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push(
                    char::from_digit((b >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((b & 0x0f) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

/// Build the agentum MCP URL for an agent, tagging it with the directory the
/// agent runs in so its `agentum_browser` ops route to the SAME per-worktree
/// browser the user's pane watches (see
/// [`crate::cdp_browser::ensure_local_cdp_browser_for`], which reduces both the
/// pane's `repoId::path` id and this bare path to one canonical key). Callers
/// pass the effective work path — the worktree when the session has one, else
/// its workdir — because a session opened in an EXISTING worktree carries no
/// `worktree_path` yet still runs in it; tagging only `worktree_path` left those
/// (the desktop's common case) untagged, so the agent silently drove the shared
/// browser and its opened tabs landed under whatever worktree the UI was focused
/// on. Empty/absent → the bare URL (contextless callers keep the shared browser).
fn mcp_url_with_worktree(base: &str, work_path: Option<&str>) -> String {
    match work_path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!("{base}/mcp?worktree={}", encode_worktree_query(p)),
        None => format!("{base}/mcp"),
    }
}

/// The path to tag the MCP URL with: the session's `worktree_path` when it has
/// one, else the directory it runs in. A session opened in an EXISTING worktree
/// carries no `worktree_path` (only sessions that ran `git worktree add` do), yet
/// still runs inside that worktree — so falling back to the workdir is what makes
/// its browser ops resolve to the right per-worktree browser instead of the
/// shared one.
fn worktree_tag_path<'a>(worktree_path: Option<&'a str>, workdir: &'a str) -> &'a str {
    worktree_path.unwrap_or(workdir)
}

/// Spawn the agent process for a freshly-(re)started session into a tmux pane
/// on `host`, arm the output pipe, and mark it `Running`. Shared by the `start`
/// HTTP handler and the harness-engine driver ([`crate::harness`]) so both go
/// through the *one* launch path — YOLO marker translation, loopback `pane_env`,
/// the Claude `--settings` PostToolUse hook, and MCP wiring all stay centralized
/// here. `workdir` must already be resolved + validated by the caller (the
/// reattach / external / worktree-heal decisions differ per caller and stay
/// out of this helper). On a pipe failure the half-spawned pane is killed so we
/// never leave an orphan behind.
pub(crate) async fn spawn_agent_into_pane(
    state: &AppState,
    session: &Session,
    host: &Host,
    target: &str,
    workdir: &std::path::Path,
) -> Result<(), ApiError> {
    let adapter = agentum_executor::adapter_for(&session.tool);
    let mut launch = adapter.launch(session);

    if matches!(host.kind, HostKind::Local) {
        // Loopback hook URLs only work for local panes. SSH-hosted agents
        // run on another machine, where 127.0.0.1 points at the VPS.
        let hook_token = crate::auth::new_token();
        for kv in pane_env(
            state.api_base_url.as_deref(),
            session.id,
            &session.name,
            &hook_token,
        ) {
            launch.env.push(kv);
        }

        if session.tool == "claude" {
            // Claude Code has no `--hook-post-tool-use` flag; hooks are
            // registered through settings. Inject a PostToolUse command hook
            // via `--settings` (which *adds* to the user's settings rather than
            // replacing them). The AGENTUM_HOOK_* refs resolve from the pane env
            // exported above, so they must stay unexpanded here.
            let hook_cmd = "curl -s -X POST \"$AGENTUM_HOOK_URL\" \
                 -H \"X-Agentum-Hook-Token: $AGENTUM_HOOK_TOKEN\" \
                 -H \"Content-Type: application/json\" \
                 -d '{\"kind\":\"tool_done\",\"payload\":{}}'";
            let settings = serde_json::json!({
                "hooks": {
                    "PostToolUse": [
                        {
                            "matcher": "*",
                            "hooks": [
                                { "type": "command", "command": hook_cmd }
                            ]
                        }
                    ]
                }
            });
            launch.argv.push("--settings".into());
            launch.argv.push(settings.to_string());
        }

        // Scope the lock so the (non-Send) MutexGuard is dropped before the
        // provisioning await below — holding it across `.await` would make the
        // caller's future non-Send and break the axum Handler bound.
        {
            let mut map = state.hook_tokens.lock().unwrap();
            map.insert(session.id, hook_token);
        }

        // Wire the agentum MCP into agents that take it via a launch arg
        // (Claude --mcp-config, Codex -c); local agents reach it on the Mac
        // loopback. Best-effort — never blocks a launch.
        let base = state
            .api_base_url
            .as_deref()
            .unwrap_or("http://127.0.0.1:8822");
        // Tag the MCP URL with this LOCAL session's worktree so the agent's
        // browser ops drive ITS per-worktree browser — the same Chromium the
        // user's pane attaches to. Use the effective work path (the resolved
        // `workdir` when there's no explicit `worktree_path`): a session opened
        // in an EXISTING worktree has no `worktree_path` but still runs in it, so
        // tagging only `worktree_path` left it untagged and its opened tabs fell
        // back to the UI-focused worktree. SSH sessions (below) keep the bare
        // URL: their browser is the host-resident one reached via a cdpPort tunnel.
        let workdir_str = workdir.to_string_lossy();
        let agentum_mcp_url = mcp_url_with_worktree(
            base,
            Some(worktree_tag_path(
                session.worktree_path.as_deref(),
                &workdir_str,
            )),
        );
        if let Some(p) =
            crate::mcp_provision::provision(state, &session.tool, &agentum_mcp_url).await
        {
            launch.argv.extend(adapter.mcp_args(&p));
            launch.env.extend(adapter.mcp_env(&p));
        }
        // File-based agents (Cursor/Gemini/OpenCode) load MCP from a config file
        // in the workdir — write it (no-op for claude/codex).
        crate::mcp_provision::write_agent_project_config(
            state,
            host,
            &workdir.to_string_lossy(),
            &session.tool,
            &agentum_mcp_url,
        )
        .await;
    } else if matches!(host.kind, HostKind::Ssh { .. }) {
        let strict_harness = session.name.starts_with("harness-");
        // Remote MCP parity: the agentum MCP lives on the Mac. Reverse-tunnel it
        // to the host (token-guarded, loopback-bound), then wire each agent at
        // the tunnel URL. Best-effort: a tunnel failure logs and launches the
        // agent without the MCP rather than blocking.
        match crate::mcp_provision::local_mcp_port(state) {
            Some(mac_port) => {
                match crate::host_runtime::ensure_reverse_tunnel(host, mac_port).await {
                    Ok(host_port) => {
                        // The remote agent needs its own orchestration handle and an
                        // AGENTUM_API_URL pointing at the tunnel.
                        launch
                            .env
                            .push(("AGENTUM_TERMINAL_HANDLE".into(), session.name.clone()));
                        launch.env.push((
                            "AGENTUM_API_URL".into(),
                            format!("http://127.0.0.1:{host_port}"),
                        ));
                        let agentum_mcp_url = format!("http://127.0.0.1:{host_port}/mcp");
                        let servers = vec![crate::mcp_provision::agentum_server(
                            state,
                            &agentum_mcp_url,
                        )];
                        let provision = if session.tool == "claude" {
                            // Claude needs the --mcp-config FILE on the HOST.
                            let host_cfg = format!("/tmp/agentum-mcp-{}.json", session.id);
                            let json = crate::mcp_provision::config_json(&servers);
                            match crate::host_runtime::write_remote_file_contained(
                                host, "/tmp", &host_cfg, &json,
                            )
                            .await
                            {
                                Ok(()) => Some(agentum_executor::McpProvision {
                                    servers,
                                    config_file: PathBuf::from(host_cfg),
                                }),
                                Err(e) => {
                                    if strict_harness {
                                        return Err(ApiError::Internal(format!(
                                            "remote harness MCP config provisioning failed: {e}"
                                        )));
                                    }
                                    tracing::warn!(session = %session.id, "could not write remote MCP config to host: {e}");
                                    None
                                }
                            }
                        } else {
                            // Codex injects MCP inline via `-c` — no host file needed.
                            Some(agentum_executor::McpProvision {
                                servers,
                                config_file: PathBuf::new(),
                            })
                        };
                        if let Some(p) = provision {
                            launch.argv.extend(adapter.mcp_args(&p));
                            launch.env.extend(adapter.mcp_env(&p));
                        }
                        // File-based agents: write the config on the HOST in the workdir.
                        if strict_harness {
                            crate::mcp_provision::write_agent_project_config_checked(
                                state,
                                host,
                                &workdir.to_string_lossy(),
                                &session.tool,
                                &agentum_mcp_url,
                            )
                            .await
                            .map_err(|e| {
                                ApiError::Internal(format!(
                                    "remote harness MCP project provisioning failed: {e}"
                                ))
                            })?;
                        } else {
                            crate::mcp_provision::write_agent_project_config(
                                state,
                                host,
                                &workdir.to_string_lossy(),
                                &session.tool,
                                &agentum_mcp_url,
                            )
                            .await;
                        }
                    }
                    Err(e) => {
                        if strict_harness {
                            return Err(ApiError::Internal(format!(
                                "remote harness MCP tunnel failed: {e}"
                            )));
                        }
                        tracing::warn!(
                            session = %session.id,
                            "reverse MCP tunnel to host failed; launching remote agent without agentum MCP: {e}"
                        );
                    }
                }
            }
            None => {
                if strict_harness {
                    return Err(ApiError::Internal(
                        "remote harness requires an embedded Agentum API endpoint".into(),
                    ));
                }
                tracing::warn!(
                    "no embedded api_base_url; cannot reverse-tunnel the agentum MCP to an SSH host"
                );
            }
        }
    }

    crate::host_runtime::new_session(host, target, workdir, &launch.argv, &launch.env)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Err(e) = crate::host_runtime::pipe_pane(host, target, &log).await {
        let _ = crate::host_runtime::kill_session(host, target).await;
        return Err(ApiError::Internal(e.to_string()));
    }

    state
        .store
        .update_status_and_target(session.id, Status::Running, Some(target))
        .await?;

    // Record what this session was provisioned with so the boot drift scan can
    // later tell whether the live endpoint moved (Local only — SSH sessions are
    // best-effort and reach agentum over a reverse tunnel, not the embedded base).
    // Best-effort: a failed record must never fail the spawn.
    if matches!(host.kind, HostKind::Local) {
        let hash = crate::mcp_provision::token_hash(state.mcp_token.as_str());
        let _ = state
            .store
            .set_session_provisioned(session.id, state.api_base_url.as_deref(), Some(&hash))
            .await;
    }
    Ok(())
}

/// Outcome of a [`reprovision_session`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reprovision {
    /// At least one config/env leg wrote new state for the current endpoint.
    Rewritten,
    /// Nothing was rewritten — endpoint couldn't be resolved (SSH tunnel down)
    /// or there was nothing to write for this tool/host.
    Skipped,
}

/// The `AGENTUM_*` env to re-apply to a live pane when the endpoint drifts.
/// Pure (no I/O) so the URL→env mapping is unit-testable without a tmux mock.
///
/// Mirrors the *connection* half of [`pane_env`] but DELIBERATELY omits the hook
/// vars unless the caller still holds the session's in-memory hook token: a
/// re-provision must not mint a fresh hook token (that would orphan the agent's
/// already-exported `AGENTUM_HOOK_TOKEN`). When `hook_token` is `None` we only
/// re-apply the API URL + orchestration handle.
pub(super) fn reprovision_env(
    api_base: Option<&str>,
    session_id: Uuid,
    session_name: &str,
    hook_token: Option<&str>,
) -> Vec<(String, String)> {
    match hook_token {
        Some(tok) => pane_env(api_base, session_id, session_name, tok),
        None => {
            let base = api_base.unwrap_or("http://127.0.0.1:8822");
            vec![
                ("AGENTUM_API_URL".to_string(), base.to_string()),
                (
                    "AGENTUM_TERMINAL_HANDLE".to_string(),
                    session_name.to_string(),
                ),
            ]
        }
    }
}

/// Re-provision a session's **agentum** MCP wiring to the *current* `state`
/// endpoint, without recreating the session. Rewrites the launch-arg combined
/// config (`<state_dir>/mcp.json`) and any file-based agent config, and re-applies
/// the `AGENTUM_*` env to the live tmux pane. Every leg is best-effort
/// (log-and-continue): a failure here must never bubble up to 500 a `/start`.
///
/// This intentionally does NOT call the full [`crate::mcp_provision::provision`]
/// (that re-spawns Playwright/CDP — N `npx` launches); it rewrites the agentum
/// entry only. tmux `set-environment` only affects *future* pane children, so the
/// already-running agent still needs to reconnect to pick up the new endpoint —
/// the rewrite makes its config current for that reconnect; we never kill it.
///
/// SSH hosts are best-effort, Local-record-only in v1: if the reverse tunnel
/// can't be resolved we return [`Reprovision::Skipped`] rather than erroring.
pub(super) async fn reprovision_session(
    state: &AppState,
    session: &Session,
    host: &Host,
) -> Reprovision {
    // 1. Resolve the endpoint this session's agent should reach agentum at.
    let url = match &host.kind {
        HostKind::Local => {
            let base = state
                .api_base_url
                .as_deref()
                .unwrap_or("http://127.0.0.1:8822");
            // Preserve the `?worktree=` tag across an endpoint-drift rewrite —
            // dropping it would silently revert this session's agent to the
            // shared browser (and its opened tabs to the UI-focused worktree).
            mcp_url_with_worktree(
                base,
                Some(worktree_tag_path(
                    session.worktree_path.as_deref(),
                    &session.workdir,
                )),
            )
        }
        HostKind::Ssh { .. } => match crate::mcp_provision::local_mcp_port(state) {
            Some(mac_port) => {
                match crate::host_runtime::ensure_reverse_tunnel(host, mac_port).await {
                    Ok(host_port) => format!("http://127.0.0.1:{host_port}/mcp"),
                    Err(e) => {
                        tracing::warn!(session = %session.id, "reprovision: reverse tunnel unavailable; skipping ({e})");
                        return Reprovision::Skipped;
                    }
                }
            }
            None => {
                tracing::warn!(session = %session.id, "reprovision: no embedded api_base_url for SSH host; skipping");
                return Reprovision::Skipped;
            }
        },
    };

    let mut rewrote = false;

    // 2. Launch-arg config: rewrite the agentum-only combined config in place.
    //    Local → the stable `<state_dir>/mcp.json`; SSH Claude → the host's
    //    `/tmp/agentum-mcp-{id}.json`; SSH Codex injects inline, nothing to write.
    let servers = vec![crate::mcp_provision::agentum_server(state, &url)];
    match &host.kind {
        HostKind::Local => match crate::mcp_provision::write_combined_config(&servers) {
            Ok(_) => rewrote = true,
            Err(e) => {
                tracing::warn!(session = %session.id, "reprovision: could not rewrite mcp.json: {e:#}")
            }
        },
        HostKind::Ssh { .. } if session.tool == "claude" => {
            let host_cfg = format!("/tmp/agentum-mcp-{}.json", session.id);
            let json = crate::mcp_provision::config_json(&servers);
            match crate::host_runtime::write_remote_file_contained(host, "/tmp", &host_cfg, &json)
                .await
            {
                Ok(()) => rewrote = true,
                Err(e) => {
                    tracing::warn!(session = %session.id, "reprovision: could not rewrite remote MCP config: {e}")
                }
            }
        }
        HostKind::Ssh { .. } => { /* Codex inline `-c`: nothing to rewrite */ }
    }

    // 3. File-based agents (Cursor/Gemini/OpenCode): merge the agentum server into
    //    the project config (no-op for claude/codex; host-aware; preserves the
    //    user's own servers).
    if crate::mcp_provision::agent_mcp_file(&session.tool).is_some() {
        crate::mcp_provision::write_agent_project_config(
            state,
            host,
            session.effective_cwd(),
            &session.tool,
            &url,
        )
        .await;
        rewrote = true;
    }

    // 4. Live pane env: re-apply the AGENTUM_* connection vars to the pane so
    //    *future* commands in it (and a reconnect) see the current endpoint.
    //    Reuse the in-memory hook token if present; never mint a fresh one.
    let hook_token = state.hook_tokens.lock().unwrap().get(&session.id).cloned();
    let target = tmux_target(session);
    for (k, v) in reprovision_env(
        state.api_base_url.as_deref(),
        session.id,
        &session.name,
        hook_token.as_deref(),
    ) {
        if let Err(e) = crate::host_runtime::set_pane_env(host, &target, &k, &v).await {
            tracing::warn!(session = %session.id, "reprovision: set_pane_env {k} failed: {e}");
        } else {
            rewrote = true;
        }
    }

    if rewrote {
        Reprovision::Rewritten
    } else {
        Reprovision::Skipped
    }
}

/// Has a session's *recorded* provisioned endpoint drifted from the live one?
/// Drift = the base URL moved OR the `/mcp` token rotated (hash differs). Pure so
/// the comparison is unit-testable without a store. `rec_base == None` is handled
/// by the caller (never provisioned → not drift); here a recorded base that
/// differs, or a recorded hash that doesn't match the live hash, is drift.
pub(super) fn endpoint_drifted(
    rec_base: &str,
    rec_hash: Option<&str>,
    live_base: &str,
    live_hash: &str,
) -> bool {
    rec_base != live_base || rec_hash != Some(live_hash)
}

/// R3: at boot, re-sync any *running* session whose recorded provisioned endpoint
/// has drifted from the live one. R1+R2 keep the port+token stable across the
/// common restart, so this is normally a no-op; it catches the residual case
/// where the persisted port was taken and the server fell back to an ephemeral
/// one (the endpoint moved out from under live sessions).
///
/// Spawned once inside [`crate::spawn_background_workers`] AFTER the live
/// `state.mcp_token` and `state.api_base_url` are final, so it reads the
/// authoritative values. Every leg is best-effort; this never blocks boot and
/// never errors out.
///
/// Scope (per PM ruling — v1 is Local-only, conservative):
/// - Standalone daemon (`api_base_url == None`) → skip entirely (no embedded
///   endpoint to drift from).
/// - Local hosts only; SSH sessions are best-effort and recorded nowhere.
/// - Skip rows with no recorded endpoint (never provisioned / not Local).
/// - Skip rows that aren't drifted (recorded == live).
/// - Gate on `has_session(...)==Ok(true)`: a Running-but-dead row is the
///   watchdog's to reconcile — we must not touch a pane that no longer exists.
///
/// For each drifted+alive session: rewrite its config/env to the live endpoint,
/// re-record the provisioned endpoint, set the `needs_reconnect` flag (the live
/// agent only re-reads MCP config at launch, so it must reconnect), and emit a
/// `session.endpoint_drifted` bus event for any connected UI.
pub(crate) async fn boot_drift_rescan(state: AppState) {
    let Some(live_base) = state.api_base_url.as_deref() else {
        return; // standalone serve(): no embedded endpoint, nothing can drift
    };
    let live_hash = crate::mcp_provision::token_hash(state.mcp_token.as_str());

    let running = match state.store.list_sessions(Some(Status::Running)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("boot drift rescan: could not list running sessions: {e}");
            return;
        }
    };

    for session in running {
        let host = match load_host_for_session(&state, &session).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(session = %session.id, "boot drift rescan: host load failed: {e}");
                continue;
            }
        };
        // Local-only in v1.
        if !matches!(host.kind, HostKind::Local) {
            continue;
        }
        // Nothing recorded → never provisioned here; leave it alone.
        let Some(rec_base) = session.provisioned_api_base.as_deref() else {
            continue;
        };
        // Drift = endpoint URL moved OR the token rotated.
        if !endpoint_drifted(
            rec_base,
            session.provisioned_token_hash.as_deref(),
            live_base,
            &live_hash,
        ) {
            continue;
        }
        // Don't touch a Running-but-dead row: if the pane is gone the watchdog
        // owns reconciling its status; re-provisioning a corpse is pointless and
        // would set a misleading needs-reconnect flag.
        let target = tmux_target(&session);
        match crate::host_runtime::has_session(&host, &target).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::warn!(session = %session.id, "boot drift rescan: has_session probe failed: {e}");
                continue;
            }
        }

        tracing::info!(
            session = %session.id,
            "endpoint drifted (recorded {rec_base} → live {live_base}); re-provisioning"
        );
        if reprovision_session(&state, &session, &host).await == Reprovision::Rewritten {
            // Re-record the now-current endpoint (this also clears the flag), then
            // raise the needs-reconnect flag: the live agent must reconnect to use
            // the rewritten config. Order matters — flag AFTER the record.
            let _ = state
                .store
                .set_session_provisioned(session.id, Some(live_base), Some(&live_hash))
                .await;
            let _ = state.store.flag_session_needs_reconnect(session.id).await;
            let _ = state.bus.send(
                Event::new("session.endpoint_drifted").with_session(session.id, session.name),
            );
        }
    }
}

/// Which sessions the boot revival sweep may respawn. Pure so it's
/// unit-testable without a store or tmux. A session qualifies when ALL hold:
///
/// - its status says it owned a live pane when the daemon last died
///   (`Running`/`Idle` — mirrors [`crate::pane_repair`]'s notion of a
///   plausibly-live row);
/// - it lives on the LOCAL host: an OS reboot/logout only kills the local
///   tmux server, SSH panes run on the remote host and survive it — and a
///   probe against an offline host would stall boot;
/// - it is not an EXTERNAL binding (that tmux session is user-owned; there
///   is nothing of ours to respawn);
/// - a respawn brings it back *faithfully*: `claude` resumes its
///   conversation via the transcript-aware adapter (`--session-id` →
///   `--resume`, see `ClaudeAdapter::launch`), and a shell (`terminal`/
///   `bash`) is stateless. Tools with no resume path (codex, cursor,
///   gemini, …) are deliberately excluded — a silently-fresh instance
///   dressed up as the old session would hide the context loss; the
///   watchdog marks them crashed instead, which the UI can surface.
fn revives_at_boot(session: &Session) -> bool {
    matches!(session.status, Status::Running | Status::Idle)
        && session.host_id.unwrap_or(LOCAL_HOST_ID) == LOCAL_HOST_ID
        && !is_external(session)
        && matches!(session.tool.as_str(), "claude" | "terminal" | "bash")
}

/// Boot revival: respawn local sessions whose tmux pane died with the OS
/// (issue #267). A reboot kills the tmux server that hosts every local pane
/// while the store still says `running`; without this sweep those rows rot —
/// the watchdog marks them crashed and the desktop's restored tabs bind dead
/// streams (which quietly fall back to a bare local shell). Respawning goes
/// through [`spawn_agent_into_pane`], the one shared launch path, so a Claude
/// session comes back with its conversation resumed.
///
/// MUST complete BEFORE the watchdog's first reconcile — the watchdog samples
/// every running session's pane and would flip these to `crashed` first.
/// [`crate::spawn_background_workers`] awaits this ahead of starting the
/// watchdog. Every leg is best-effort: a session that can't be revived
/// (missing workdir, tmux failure, lost spawn race against an early UI
/// `/start`) is logged and left for the watchdog to reconcile exactly as
/// before this sweep existed.
pub(crate) async fn boot_revive_dead_sessions(state: &AppState) {
    let all = match state.store.list_sessions(None).await {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = ?e, "boot revival: session listing failed; skipping sweep");
            return;
        }
    };

    for session in all.iter().filter(|s| revives_at_boot(s)) {
        let host = match load_host_for_session(state, session).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(session = %session.id, "boot revival: host load failed: {e}");
                continue;
            }
        };
        // `revives_at_boot` gated on the local host *id*; keep the kind check
        // so a mislabeled host row can't route tmux calls somewhere remote.
        if !matches!(host.kind, HostKind::Local) {
            continue;
        }

        let target = tmux_target(session);
        match crate::host_runtime::has_session(&host, &target).await {
            // Pane survived (plain app restart, not a reboot) — leave it be;
            // `start` reattaches lazily when a client connects.
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(session = %session.id, "boot revival: has_session probe failed: {e}");
                continue;
            }
        }

        let workdir = match crate::routes::util::expand_workdir(session.effective_cwd()) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(session = %session.id, "boot revival: bad workdir: {e}");
                continue;
            }
        };
        if let Err(e) = self_heal_local_workdir(&workdir).await {
            tracing::warn!(session = %session.id, "boot revival: workdir gone: {e}");
            continue;
        }

        match spawn_agent_into_pane(state, session, &host, &target, &workdir).await {
            Ok(()) => {
                tracing::info!(
                    session = %session.id, name = %session.name, tool = %session.tool,
                    "boot revival: respawned session whose pane died with the tmux server"
                );
                let _ = state
                    .bus
                    .send(Event::new("session.revived").with_session(session.id, &session.name));
            }
            Err(e) => {
                tracing::warn!(session = %session.id, "boot revival: respawn failed: {e}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_env_publishes_api_url_and_derives_hook_from_it() {
        let sid = uuid::Uuid::nil();
        let env = pane_env(Some("http://127.0.0.1:5544"), sid, "build-agent", "tok");
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("AGENTUM_API_URL"), Some("http://127.0.0.1:5544"));
        // Hook URL is anchored to the SAME base — never a separate hardcoded 8822.
        assert_eq!(
            get("AGENTUM_HOOK_URL"),
            Some(format!("http://127.0.0.1:5544/api/sessions/{sid}/hook").as_str())
        );
        assert_eq!(get("AGENTUM_HOOK_TOKEN"), Some("tok"));
        // The orchestration handle is the session name.
        assert_eq!(get("AGENTUM_TERMINAL_HANDLE"), Some("build-agent"));
    }

    #[test]
    fn pane_env_falls_back_to_8822_when_base_unknown() {
        // A standalone daemon (no embedded api_base_url) keeps the conventional port.
        let env = pane_env(None, uuid::Uuid::nil(), "sh", "tok");
        let url = env.iter().find(|(k, _)| k == "AGENTUM_API_URL").unwrap();
        assert_eq!(url.1, "http://127.0.0.1:8822");
    }

    #[test]
    fn mcp_url_tags_worktree_so_agent_and_pane_share_a_browser() {
        let base = "http://127.0.0.1:8822";
        // A worktree session: the URL carries `?worktree=<path>` so the `/mcp`
        // handler routes this agent's browser ops to its own Chromium. The id the
        // pane sends (`<repoId>::<path>`) canonicalizes to this SAME path
        // server-side, so agent and pane attach to one browser.
        assert_eq!(
            mcp_url_with_worktree(base, Some("/Users/x/.agentum/worktrees/feat")),
            "http://127.0.0.1:8822/mcp?worktree=/Users/x/.agentum/worktrees/feat",
        );
        // A space (or any non-unreserved byte) is percent-encoded so axum's
        // serde_urlencoded decodes the value back intact.
        assert_eq!(
            mcp_url_with_worktree(base, Some("/Users/My Name/wt")),
            "http://127.0.0.1:8822/mcp?worktree=/Users/My%20Name/wt",
        );
        // No worktree (or blank) → the bare URL: contextless agents keep the
        // shared browser, byte-identical to the pre-change behavior.
        assert_eq!(
            mcp_url_with_worktree(base, None),
            "http://127.0.0.1:8822/mcp"
        );
        assert_eq!(
            mcp_url_with_worktree(base, Some("  ")),
            "http://127.0.0.1:8822/mcp"
        );
    }

    #[test]
    fn worktree_tag_path_falls_back_to_workdir_for_existing_worktree_sessions() {
        // An explicit worktree (a `git worktree add` session) wins.
        assert_eq!(
            worktree_tag_path(Some("/repo/.claude/worktrees/feat"), "/repo"),
            "/repo/.claude/worktrees/feat"
        );
        // The regression this fix targets: a session OPENED in an existing
        // worktree has no `worktree_path`, so we must tag its workdir — otherwise
        // it stays untagged and its browser falls back to the UI-active worktree.
        assert_eq!(
            worktree_tag_path(None, "/repo/.claude/worktrees/feat"),
            "/repo/.claude/worktrees/feat"
        );
    }

    #[test]
    fn reprovision_env_with_hook_token_reapplies_full_connection_env() {
        // When the in-memory hook token survives, a re-provision re-applies the
        // SAME vars `pane_env` exported at launch — anchored to the NEW base.
        let sid = uuid::Uuid::nil();
        let env = reprovision_env(Some("http://127.0.0.1:9001"), sid, "dbg", Some("htok"));
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("AGENTUM_API_URL"), Some("http://127.0.0.1:9001"));
        assert_eq!(
            get("AGENTUM_HOOK_URL"),
            Some(format!("http://127.0.0.1:9001/api/sessions/{sid}/hook").as_str())
        );
        // The existing hook token is reused verbatim — never re-minted.
        assert_eq!(get("AGENTUM_HOOK_TOKEN"), Some("htok"));
        assert_eq!(get("AGENTUM_TERMINAL_HANDLE"), Some("dbg"));
    }

    #[test]
    fn reprovision_env_without_hook_token_skips_hook_vars() {
        // No surviving hook token → only the API URL + handle are re-applied;
        // we must NOT emit AGENTUM_HOOK_* (that would imply a fresh token we
        // deliberately never mint on re-provision).
        let env = reprovision_env(
            Some("http://127.0.0.1:9001"),
            uuid::Uuid::nil(),
            "dbg",
            None,
        );
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"AGENTUM_API_URL"));
        assert!(keys.contains(&"AGENTUM_TERMINAL_HANDLE"));
        assert!(!keys.contains(&"AGENTUM_HOOK_URL"));
        assert!(!keys.contains(&"AGENTUM_HOOK_TOKEN"));
    }

    #[test]
    fn reprovision_env_falls_back_to_8822_when_base_unknown() {
        let env = reprovision_env(None, uuid::Uuid::nil(), "sh", None);
        let url = env.iter().find(|(k, _)| k == "AGENTUM_API_URL").unwrap();
        assert_eq!(url.1, "http://127.0.0.1:8822");
    }

    #[test]
    fn endpoint_drift_detects_port_and_token_changes() {
        let live_base = "http://127.0.0.1:8822";
        let live_hash = "abc123";
        // Same base + same token → no drift (the R1+R2 common-restart case).
        assert!(!endpoint_drifted(
            live_base,
            Some(live_hash),
            live_base,
            live_hash
        ));
        // Base moved (ephemeral rebind) → drift.
        assert!(endpoint_drifted(
            "http://127.0.0.1:60102",
            Some(live_hash),
            live_base,
            live_hash
        ));
        // Token rotated (hash differs) → drift.
        assert!(endpoint_drifted(
            live_base,
            Some("stale-hash"),
            live_base,
            live_hash
        ));
        // Recorded base matches but no recorded hash → treat as drift (re-sync to
        // be safe rather than leave a half-recorded row stale).
        assert!(endpoint_drifted(live_base, None, live_base, live_hash));
    }

    /// Build a `Session` through serde so the helper stays valid as optional
    /// fields are added to the struct (same approach as `pane_repair`'s tests).
    fn sess(tool: &str, status: &str, host: Option<&str>, external: bool) -> Session {
        serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(),
            "name": "some-session",
            "workdir": "/tmp",
            "tool": tool,
            "model": null,
            "flags": if external { vec![agentum_core::EXTERNAL_TMUX_FLAG.to_string()] } else { vec![] },
            "status": status,
            "tmux_target": null,
            "host_id": host,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "last_activity_at": null,
        }))
        .unwrap()
    }

    #[test]
    fn boot_revival_targets_local_claude_and_shell_sessions() {
        // The reboot case this sweep exists for: a local Claude session whose
        // row still says running — revive (the adapter resumes its transcript).
        assert!(revives_at_boot(&sess("claude", "running", None, false)));
        // Shells are stateless; a respawned pane IS the session, faithfully.
        assert!(revives_at_boot(&sess("terminal", "running", None, false)));
        assert!(revives_at_boot(&sess("bash", "running", None, false)));
        // `Idle` also plausibly owned a live pane (pane_repair's notion too).
        assert!(revives_at_boot(&sess("claude", "idle", None, false)));
    }

    #[test]
    fn boot_revival_never_fakes_a_non_resumable_agent() {
        // codex/cursor/gemini have no conversation-resume path: a fresh
        // instance silently dressed up as the old session would hide the
        // context loss. Leave them for the watchdog to mark crashed.
        assert!(!revives_at_boot(&sess("codex", "running", None, false)));
        assert!(!revives_at_boot(&sess("cursor", "running", None, false)));
        assert!(!revives_at_boot(&sess("gemini", "running", None, false)));
    }

    #[test]
    fn boot_revival_skips_remote_external_and_settled_sessions() {
        // SSH panes live on the remote host and survive a Mac reboot.
        assert!(!revives_at_boot(&sess(
            "claude",
            "running",
            Some("4bfb2ccf-cdd0-4a82-8793-5d87906da5e0"),
            false
        )));
        // External tmux sessions are user-owned — nothing of ours to respawn.
        assert!(!revives_at_boot(&sess("terminal", "running", None, true)));
        // Stopped/crashed rows settled deliberately (or a prior boot already
        // reconciled them) — reviving those is the user's explicit call.
        assert!(!revives_at_boot(&sess("claude", "stopped", None, false)));
        assert!(!revives_at_boot(&sess("claude", "crashed", None, false)));
        // An explicit LOCAL_HOST_ID is equivalent to no host id.
        assert!(revives_at_boot(&sess(
            "claude",
            "running",
            Some("00000000-0000-0000-0000-000000000000"),
            false
        )));
    }
}
