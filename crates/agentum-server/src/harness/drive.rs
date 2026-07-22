//! The harness drive loop + orchestration: init → per-feature (spawn agent →
//! settle → verify gate → QA gate → advance/retry/block) → review, plus the
//! SDD role-gate phases, agent-session spawning, REPL readiness/prompt-injection,
//! and settle detection. These free functions drive a `HarnessEngine` (held in
//! `AppState`); as a child module they can call its private methods directly.

use std::path::Path;
use std::time::{Duration, Instant};

use agentum_core::{Event, HostKind, LOCAL_HOST_ID, NewSession, Status};
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

use super::helpers::*;
use super::types::*;
use crate::AppState;

/// How long to let the agent CLI boot its REPL before typing the prompt in.
const AGENT_BOOT_DELAY: Duration = Duration::from_secs(3);

/// Drive a harness end-to-end: init → per-feature (spawn agent → settle →
/// verify gate → advance/retry/block) → done. Spawned as a background task by
/// `POST /api/harness/{id}/run`. Holds a full [`AppState`] so it can create and
/// start REAL agent sessions through the same launch path the `start` route
/// uses. All errors are surfaced as `HarnessEvent::Error` + a `Failed` state
/// rather than panicking the task.
pub async fn drive(state: AppState, harness_id: Uuid) {
    let result = drive_inner(&state, harness_id).await;
    // Free the driver slot no matter how the loop ended so the run can be
    // re-driven (after done/blocked/failed, or once the user re-runs).
    state.harness.release_driver(harness_id).await;
    if let Err(e) = result {
        // A run removed mid-drive (user pressed Stop/unload) surfaces here as a
        // "harness not found" error — that's an intentional teardown, not a
        // failure. Only surface a real error if the run still exists.
        if let Ok(status) = state.harness.status(harness_id).await {
            warn!(%harness_id, error = %e, "harness run failed");
            state.harness.emit_error(harness_id, e.to_string());
            // A failed spawn/delivery must not leave a dead session pinned as
            // the run's current surface. The persistent progress bar remains
            // visible and can retry the run, while the stale pane stops looking
            // like work is still in progress.
            if let Some(session_id) = status.current_session
                && let Ok(Some(session)) = state.store.get_session_by_id(session_id).await
            {
                teardown_session(&state, &session).await;
            }
            state.harness.clear_current(harness_id).await;
            let _ = state
                .harness
                .set_state(harness_id, HarnessState::Failed)
                .await;
        }
    }
}

async fn drive_inner(state: &AppState, harness_id: Uuid) -> anyhow::Result<()> {
    let engine = &state.harness;

    // 1. Environment smoke-test.
    engine.log(harness_id, None, "running init.sh");
    if !engine.run_init(harness_id).await? {
        anyhow::bail!("init.sh failed — environment not ready");
    }

    engine.set_state(harness_id, HarnessState::Running).await?;

    // A previous run may have halted with a feature `Blocked`. On (re-)drive,
    // retry it from `Pending` rather than skipping past it — `next_pending_feature`
    // ignores `Blocked`, so without this the run would jump to the next feature
    // and quietly abandon the one that actually failed the gate.
    let reset = engine.reset_blocked_features(harness_id).await?;
    if reset > 0 {
        engine.log(
            harness_id,
            None,
            format!("reset {reset} blocked feature(s) for retry"),
        );
    }

    let workdir = engine.workdir(harness_id).await?;

    // 1b. SDD role phases wrap the feature loop (spec 013). OFF by default → a
    // plain feature run skips straight to the loop below, behaving exactly as
    // before. When on, PM/architect gates + decompose run first; the feature
    // loop is the `Executing` phase; the reviewer gate runs after all are green.
    let phases_on = {
        let cfg = HarnessConfig::load(&workdir).await?;
        cfg.features.roles && cfg.features.spec_id.is_some()
    };
    if phases_on {
        run_pre_feature_phases(state, harness_id, &workdir).await?;
        // A role gate (or decompose) may have halted the run — stop unless it
        // reached the feature-execution phase.
        if engine.phase(harness_id).await? != SpecPhase::Executing {
            return Ok(());
        }
    }

    // 2. One feature at a time.
    loop {
        let Some(feature) = engine.next_pending_feature(harness_id).await? else {
            // All features green. With role phases on, the reviewer gate is the
            // last gate before Done; a blocked/parked review halts here without
            // claiming success.
            if phases_on {
                run_review_phase(state, harness_id, &workdir).await?;
                if engine.phase(harness_id).await? != SpecPhase::Done {
                    return Ok(());
                }
            }
            engine.clear_current(harness_id).await;
            engine.set_state(harness_id, HarnessState::Done).await?;
            engine.emit(HarnessEvent::HarnessCompleted {
                harness_id,
                success: true,
            });
            engine.log(harness_id, None, "all features verified — harness done");
            return Ok(());
        };

        engine.log(
            harness_id,
            Some(&feature.id),
            format!("starting feature: {}", feature.name),
        );
        // Reset the run-level state to `Running` for each feature: the previous
        // feature left it at `Verifying` after its green gate, and the UI gate
        // banner keys off this — without it an actively-coding agent would show
        // as "VERIFYING…".
        engine.set_state(harness_id, HarnessState::Running).await?;

        // 3. Reload config (agent tool/model/timeouts may have been edited).
        let config = HarnessConfig::load(&workdir).await?;
        let session = spawn_feature_agent(state, harness_id, &workdir, &config, &feature).await?;
        // The agent is now coding this feature → move its ticket to "In Progress"
        // (best-effort; never halts the run).
        transition_tracker(
            state,
            harness_id,
            &feature,
            crate::task_sink::TrackerPhase::InProgress,
        )
        .await;

        // `wait_for_settle` subscribes to the lifecycle bus on entry (just after
        // the prompt is injected). The `grace` window covers the agent's initial
        // idle so we never gate before it has had a chance to act; a settle
        // signal inside that window is remembered, not discarded.
        let grace = Duration::from_secs(config.features.settle_grace_secs);
        let timeout = Duration::from_secs(config.features.settle_timeout_secs);

        // 4. Hand the agent its scoped task. `inject_prompt` waits for the REPL
        // to be ready first (accepting Claude's workspace-trust dialog and
        // outlasting an MCP-slowed boot), so there's no fixed-delay guesswork.
        // Only steer at a spec that actually exists on disk — a stale spec_id
        // must not send the agent hunting for a missing file. Handles the legacy
        // `.harness` dir by deriving the dir name from config.harness_dir, not
        // the HARNESS_DIR const (spec 005 F2).
        let spec_rel = config.features.spec_id.as_deref().and_then(|sid| {
            let dir = config
                .harness_dir
                .file_name()?
                .to_string_lossy()
                .into_owned();
            let rel = format!("{dir}/specs/{sid}/spec.md");
            workdir.join(&rel).exists().then_some(rel)
        });
        let prompt =
            build_feature_prompt(&config.agent_instructions, &feature, spec_rel.as_deref());
        if !inject_prompt(state, &session, &prompt).await? {
            engine.log(harness_id, Some(&feature.id), repl_not_ready_message());
        }
        engine.log(harness_id, Some(&feature.id), "agent working…");
        if wait_for_settle(&state.bus, session.id, grace, timeout).await == SettleOutcome::TimedOut
        {
            engine.log(
                harness_id,
                Some(&feature.id),
                settle_timeout_message(timeout),
            );
        }

        // 5. Two-phase gate with retry (spec 012). First the unit-test gate
        //    (verify.sh), then the browser QA gate (qa.sh / browser-verification-
        //    loop). A red gate at EITHER phase hands the error back to the agent
        //    and retries; only when BOTH are green does the feature advance.
        loop {
            engine.log(
                harness_id,
                Some(&feature.id),
                "running unit-test gate (verify.sh)",
            );
            let (unit_ok, unit_out) = engine.run_verify_once(harness_id, &feature.id).await?;
            if !unit_ok {
                if handle_gate_failure(
                    state,
                    harness_id,
                    &feature,
                    &session,
                    "unit-test gate (verify.sh)",
                    &unit_out,
                    grace,
                    timeout,
                )
                .await?
                {
                    return Ok(()); // retries exhausted → run halted at Blocked
                }
                continue;
            }

            // Unit gate green → "Ready to Test": flip the ticket column, then run
            // the browser QA gate. `run_qa_once` also moves the feature state to
            // `ReadyToTest` so the in-app board matches.
            transition_tracker(
                state,
                harness_id,
                &feature,
                crate::task_sink::TrackerPhase::ReadyToTest,
            )
            .await;
            // Pick the QA gate: a spawned browser QA agent (012b) or the `qa.sh`
            // shell gate. `Auto` prefers qa.sh, else an agent when agent-QA is
            // capable, else the skip-pass script path. Capability = the
            // AGENTUM_BROWSER_VERIFY env flag OR the Settings knob (spec 005 F3,
            // default OFF) — computed here, where AppState lives, so
            // `resolve_qa_mode` stays a pure decision table.
            let agent_qa_capable =
                crate::playwright_mcp::feature_enabled() || browser_qa_agent_enabled(state).await;
            let qa_mode = resolve_qa_mode(&config, agent_qa_capable);
            let (qa_ok, qa_out) = match qa_mode {
                QaMode::Agent => {
                    let result = run_qa_agent_gate(
                        state, harness_id, &workdir, &config, &feature, grace, timeout,
                    )
                    .await?;
                    // The QA pane is intentionally torn down once its verdict
                    // lands. Restore the still-live coding session as current
                    // so retries/SDD controls return to the agent that receives
                    // gate feedback.
                    engine
                        .set_current_session(
                            harness_id,
                            session.id,
                            &config.features.agent_tool,
                            Some(&feature.id),
                        )
                        .await?;
                    result
                }
                _ => engine.run_qa_once(harness_id, &feature.id).await?,
            };
            let qa_label = if qa_mode == QaMode::Agent {
                "browser QA gate (agent)"
            } else {
                "browser QA gate (qa.sh)"
            };
            if !qa_ok {
                if handle_gate_failure(
                    state, harness_id, &feature, &session, qa_label, &qa_out, grace, timeout,
                )
                .await?
                {
                    return Ok(());
                }
                continue;
            }

            // Both gates green.
            if engine.hitl_at_qa(harness_id).await? {
                // HITL-at-QA: pause for ONE human confirmation before Done.
                engine.await_confirm(harness_id, &feature.id).await?;
                engine.log(
                    harness_id,
                    Some(&feature.id),
                    "✓ unit + QA gates PASSED — awaiting human confirmation (HITL-at-QA). Run paused; POST /{id}/confirm to finalize and resume.",
                );
                return Ok(());
            }
            let summary = format!(
                "unit gate:\n{}\n\nQA gate:\n{}",
                tail(&unit_out, 1000),
                tail(&qa_out, 1000)
            );
            engine
                .mark_feature_done(harness_id, &feature.id, &summary)
                .await?;
            // Both gates green → ticket Done.
            transition_tracker(
                state,
                harness_id,
                &feature,
                crate::task_sink::TrackerPhase::Done,
            )
            .await;
            engine.log(
                harness_id,
                Some(&feature.id),
                "✓ unit + QA gates PASSED — feature done",
            );
            break;
        }

        // 6. Feature is done — tear down its agent pane before the next one.
        teardown_session(state, &session).await;
    }
}

/// Handle a red gate (unit or QA): record the failure and either halt the run
/// (`Ok(true)` — retries exhausted, feature `Blocked`) or hand the error back to
/// the agent and wait for it to settle for another attempt (`Ok(false)`).
/// `gate_label` names the failing gate in the log + retry prompt. Shared by both
/// phases so the retry behavior is identical (spec 012).
#[allow(clippy::too_many_arguments)]
async fn handle_gate_failure(
    state: &AppState,
    harness_id: Uuid,
    feature: &Feature,
    session: &agentum_core::Session,
    gate_label: &str,
    output: &str,
    grace: Duration,
    timeout: Duration,
) -> anyhow::Result<bool> {
    let engine = &state.harness;
    let (blocked, attempts) = engine
        .record_feature_failure(harness_id, &feature.id, output)
        .await?;
    if blocked {
        engine.set_state(harness_id, HarnessState::Blocked).await?;
        engine.log(
            harness_id,
            Some(&feature.id),
            format!("✗ {gate_label} FAILED — retries exhausted, feature BLOCKED. Run halted."),
        );
        // Spec 008 F1 #16 (D6): the in-app side is already loud — make the ISSUE
        // loud too. Escalate with a `status/blocked` label + a comment carrying
        // the retry count and the gate-output tail. Best-effort by contract
        // (`apply_blocked_transition` never `Err`s for a tracker hiccup): log
        // both Ok and Err non-fatally — a blocked issue-update never un-halts or
        // re-halts the already-halted run.
        if let Some(provider) = feature.tracker_provider.as_deref() {
            match crate::task_sink::apply_blocked_transition(
                &state.store,
                provider,
                &feature.id,
                feature.tracker_url.as_deref(),
                &feature.name,
                gate_label,
                attempts,
                &tail(output, 2000),
                // Retries-exhausted always explains itself on the issue; only
                // the spec-014 attention worker suppresses inside a cooldown.
                /* with_comment */
                true,
                crate::task_sink::TrackerEmit {
                    bus: &state.bus,
                    worktree_id: None,
                },
            )
            .await
            {
                Ok(r) => engine.log(
                    harness_id,
                    Some(&feature.id),
                    format!("blocked → issue: {r:?}"),
                ),
                Err(e) => engine.log(
                    harness_id,
                    Some(&feature.id),
                    format!("blocked issue update failed (non-fatal): {e}"),
                ),
            }
        }
        // Leave the agent session alive so the user can intervene.
        return Ok(true);
    }
    engine.log(
        harness_id,
        Some(&feature.id),
        format!("✗ {gate_label} FAILED — handing the error back to the agent for a retry"),
    );
    engine.set_state(harness_id, HarnessState::Running).await?;
    let retry = format!(
        "The {gate_label} FAILED with this output:\n\n{}\n\n\
         Fix the problem for feature '{}' and stop when done — the gate will run again.",
        tail(output, 2000),
        feature.name,
    );
    if !inject_prompt(state, session, &retry).await? {
        engine.log(harness_id, Some(&feature.id), repl_not_ready_message());
    }
    if wait_for_settle(&state.bus, session.id, grace, timeout).await == SettleOutcome::TimedOut {
        engine.log(
            harness_id,
            Some(&feature.id),
            settle_timeout_message(timeout),
        );
    }
    Ok(false)
}

/// Drive the feature's ticket to a pipeline phase in whatever tracker it came
/// from (spec 012). A side-channel: the result (applied / skipped / error) is
/// only logged — a tracker hiccup NEVER halts the harness run. A feature with no
/// `tracker_provider` (e.g. a hand-written backlog) is a silent no-op.
async fn transition_tracker(
    state: &AppState,
    harness_id: Uuid,
    feature: &Feature,
    phase: crate::task_sink::TrackerPhase,
) {
    let Some(provider) = feature.tracker_provider.as_deref() else {
        return;
    };
    let engine = &state.harness;
    let result = crate::task_sink::apply_tracker_transition(
        &state.store,
        provider,
        &feature.id,
        feature.tracker_url.as_deref(),
        phase,
        crate::task_sink::TrackerEmit {
            bus: &state.bus,
            worktree_id: None,
        },
    )
    .await;
    match &result {
        Ok(crate::task_sink::TransitionResult::Applied) => {
            engine.log(harness_id, Some(&feature.id), format!("ticket → {phase:?}"))
        }
        Ok(crate::task_sink::TransitionResult::Skipped(why)) => engine.log(
            harness_id,
            Some(&feature.id),
            format!("ticket transition to {phase:?} skipped: {why}"),
        ),
        Err(e) => engine.log(
            harness_id,
            Some(&feature.id),
            format!("ticket transition to {phase:?} failed (non-fatal): {e}"),
        ),
    }
    if !matches!(result, Ok(crate::task_sink::TransitionResult::Applied)) {
        spawn_tracker_transition_retry(state, harness_id, feature, phase);
    }
}

/// Keep an unacknowledged lifecycle write pending without stalling the harness.
/// Five capped-backoff attempts cover transient auth/network/Projects failures;
/// every failure emits `tracker.sync_pending`, and later lifecycle observers
/// (session/PR/merge) remain able to retry beyond this bounded worker.
fn spawn_tracker_transition_retry(
    state: &AppState,
    harness_id: Uuid,
    feature: &Feature,
    phase: crate::task_sink::TrackerPhase,
) {
    let Some(provider) = feature.tracker_provider.clone() else {
        return;
    };
    let tracker_id = feature.id.clone();
    let tracker_url = feature.tracker_url.clone();
    let store = state.store.clone();
    let bus = state.bus.clone();
    let engine = state.harness.clone();
    tokio::spawn(async move {
        for attempt in 1..=5u32 {
            let delay = std::time::Duration::from_secs(2u64.saturating_pow(attempt).min(60));
            tokio::time::sleep(delay).await;
            let result = crate::task_sink::apply_tracker_transition(
                &store,
                &provider,
                &tracker_id,
                tracker_url.as_deref(),
                phase,
                crate::task_sink::TrackerEmit {
                    bus: &bus,
                    worktree_id: None,
                },
            )
            .await;
            if matches!(result, Ok(crate::task_sink::TransitionResult::Applied)) {
                engine.log(
                    harness_id,
                    Some(&tracker_id),
                    format!("ticket → {phase:?} (sync retry {attempt})"),
                );
                return;
            }
            tracing::warn!(
                feature = %tracker_id,
                ?phase,
                attempt,
                ?result,
                "tracker transition remains pending"
            );
        }
    });
}

/// A per-SPAWN unique, tmux-safe session name: `harness-<kind>-<run8>-<nonce4>`.
///
/// The nonce is load-bearing: `sessions.name` is UNIQUE and harness session
/// rows are only marked stopped (never deleted), so a role-gate retry, a QA
/// re-run, or a re-drive of an existing run that reused the previous spawn's
/// name would fail `create_session_on_host` with AlreadyExists and take the
/// WHOLE run to `Failed` (#302). `kind` is clamped so the name stays within
/// `validate_name`'s 64-char cap (safe to byte-slice: inputs are ASCII —
/// `sanitize` output and role slugs).
pub(super) fn spawn_session_name(kind: &str, harness_id: Uuid) -> String {
    let kind = &kind[..kind.len().min(40)];
    let run = harness_id.simple().to_string();
    let nonce = Uuid::new_v4().simple().to_string();
    format!("harness-{kind}-{}-{}", &run[..8], &nonce[..4])
}

/// Create + start a real agent session scoped to one feature.
async fn spawn_feature_agent(
    state: &AppState,
    harness_id: Uuid,
    workdir: &Path,
    config: &HarnessConfig,
    feature: &Feature,
) -> anyhow::Result<agentum_core::Session> {
    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| anyhow::anyhow!("local host missing"))?;

    let name = spawn_session_name(&sanitize(&feature.id), harness_id);

    // The harness is non-interactive — push the canonical YOLO marker so the
    // agent runs without permission prompts (the shared spawn path translates it
    // to each tool's flag). Without this the agent stalls on the first prompt and
    // never reaches the gate. See CLAUDE.md "YOLO marker translation".
    let flags = if config.features.agent_yolo {
        vec![agentum_executor::YOLO_MARKER.to_string()]
    } else {
        Vec::new()
    };

    let new = NewSession {
        name: name.clone(),
        workdir: workdir.to_string_lossy().into_owned(),
        tool: config.features.agent_tool.clone(),
        model: config.features.agent_model.clone(),
        flags,
        card_id: None,
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };
    let session = state
        .store
        .create_session_on_host(new, Some(LOCAL_HOST_ID))
        .await?;

    state
        .harness
        .set_session(
            harness_id,
            session.id,
            &feature.id,
            &config.features.agent_tool,
        )
        .await?;

    let target = agentum_tmux::target_for(&session.name);
    crate::routes::sessions::spawn_agent_into_pane(state, &session, &host, &target, workdir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn agent: {e}"))?;

    info!(%harness_id, feature = %feature.id, session = %session.id, "harness spawned agent");
    Ok(session)
}

/// Resolve the configured [`QaMode`] to a concrete gate for this run. `Auto`
/// prefers an explicit `qa.sh`, else a QA agent when `agent_qa_capable`, else
/// the (skip-pass) script path. Pure — the env/setting reads moved to the
/// caller (spec 005 F3), so the full mode × qa.sh × capability decision table
/// is unit-testable without env mutation.
pub(super) fn resolve_qa_mode(config: &HarnessConfig, agent_qa_capable: bool) -> QaMode {
    match config.features.qa_mode {
        QaMode::Script => QaMode::Script,
        QaMode::Agent => QaMode::Agent,
        QaMode::Auto => {
            if config.qa_script.is_some() {
                QaMode::Script
            } else if agent_qa_capable {
                QaMode::Agent
            } else {
                // No qa.sh and agent-QA not capable → the script path returns a
                // skip-pass, so a non-web project isn't blocked.
                QaMode::Script
            }
        }
    }
}

/// Best-effort read of the Settings browser-QA knob (mirrors
/// `routes/mcp.rs::orchestration_enabled`): a store error falls back to OFF —
/// never a run failure, and OFF is the D3 default (spec 005 F3).
async fn browser_qa_agent_enabled(state: &AppState) -> bool {
    state
        .store
        .setting_get_bool(crate::routes::harness::BROWSER_QA_ENABLED_SETTING, false)
        .await
        .unwrap_or(false)
}

/// Spawn a real agent session for the **browser QA gate** (spec 012b). Mirrors
/// [`spawn_feature_agent`] but does NOT bind the feature to the session or flip
/// it to `Coding` — the feature is already `ReadyToTest` and the coding agent is
/// torn down by now. Uses `qa_agent_tool` (default = the feature agent tool).
async fn spawn_qa_agent(
    state: &AppState,
    harness_id: Uuid,
    workdir: &Path,
    config: &HarnessConfig,
    feature: &Feature,
) -> anyhow::Result<agentum_core::Session> {
    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| anyhow::anyhow!("local host missing"))?;
    let name = spawn_session_name(&format!("qa-{}", sanitize(&feature.id)), harness_id);
    let flags = if config.features.agent_yolo {
        vec![agentum_executor::YOLO_MARKER.to_string()]
    } else {
        Vec::new()
    };
    let tool = config
        .features
        .qa_agent_tool
        .clone()
        .unwrap_or_else(|| config.features.agent_tool.clone());
    let new = NewSession {
        name: name.clone(),
        workdir: workdir.to_string_lossy().into_owned(),
        tool,
        model: config.features.agent_model.clone(),
        flags,
        card_id: None,
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };
    let session = state
        .store
        .create_session_on_host(new, Some(LOCAL_HOST_ID))
        .await?;
    let target = agentum_tmux::target_for(&session.name);
    crate::routes::sessions::spawn_agent_into_pane(state, &session, &host, &target, workdir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn QA agent: {e}"))?;
    state
        .harness
        .set_current_session(harness_id, session.id, &session.tool, Some(&feature.id))
        .await?;
    info!(%harness_id, feature = %feature.id, session = %session.id, "harness spawned QA agent");
    Ok(session)
}

/// The agent-driven browser QA gate (spec 012b): flip the feature to
/// `ReadyToTest`, spawn a browser-verification-loop agent, wait for it to settle,
/// then read its verdict file. Returns `(passed, summary)` exactly like
/// [`HarnessEngine::run_qa_once`] so the driver's retry loop is unchanged. A
/// missing/garbled verdict is a **fail** — an inconclusive QA never advances a
/// feature to Done.
async fn run_qa_agent_gate(
    state: &AppState,
    harness_id: Uuid,
    workdir: &Path,
    config: &HarnessConfig,
    feature: &Feature,
    grace: Duration,
    timeout: Duration,
) -> anyhow::Result<(bool, String)> {
    let engine = &state.harness;
    engine
        .set_feature_state(harness_id, &feature.id, FeatureState::ReadyToTest)
        .await?;

    // Verdict file under <harness_dir>/qa/<id>.json; clear any stale one first so
    // we never read a previous attempt's result.
    let verdict_abs = qa_verdict_path(&config.harness_dir, &feature.id);
    if let Some(parent) = verdict_abs.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    tokio::fs::remove_file(&verdict_abs).await.ok();
    let verdict_rel = verdict_abs
        .strip_prefix(workdir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| verdict_abs.to_string_lossy().into_owned());

    // The QA agent drives `agentum_browser` (wired by default via the agentum
    // MCP); warn only when the MCP master switch is OFF — then the agent has no
    // browser tool at all (spec 005 F3 — replaces the stale Playwright warning).
    if !state
        .store
        .setting_get_bool(crate::routes::mcp::MCP_ENABLED_SETTING, true)
        .await
        .unwrap_or(true)
    {
        engine.log(
            harness_id,
            Some(&feature.id),
            "QA agent: the agentum MCP master switch is OFF (Settings → Agent MCP) — the agent has no `agentum_browser` tool and the QA gate will likely fail.",
        );
    }
    engine.log(
        harness_id,
        Some(&feature.id),
        "unit gate green — spawning browser QA agent (agentum_browser)",
    );

    let session = spawn_qa_agent(state, harness_id, workdir, config, feature).await?;
    let prompt = build_qa_prompt(&config.agent_instructions, feature, &verdict_rel);
    if !inject_prompt(state, &session, &prompt).await? {
        engine.log(harness_id, Some(&feature.id), repl_not_ready_message());
    }
    engine.log(
        harness_id,
        Some(&feature.id),
        "QA agent verifying in browser…",
    );
    if wait_for_settle(&state.bus, session.id, grace, timeout).await == SettleOutcome::TimedOut {
        engine.log(
            harness_id,
            Some(&feature.id),
            settle_timeout_message(timeout),
        );
    }
    teardown_session(state, &session).await;

    match tokio::fs::read_to_string(&verdict_abs).await {
        Ok(raw) => match parse_qa_verdict(&raw) {
            Ok((passed, summary)) => Ok((passed, summary)),
            Err(e) => Ok((
                false,
                format!("QA verdict unreadable — failing the gate: {e}"),
            )),
        },
        Err(_) => Ok((
            false,
            format!(
                "QA agent wrote no verdict at {verdict_rel} — treating QA as failed (inconclusive)."
            ),
        )),
    }
}

/// Read the spec under review for a role gate (spec 013). Best-effort: an empty
/// string if the spec is absent (the gate will then most likely emit CONCERNS).
async fn read_spec_md(harness_dir: &Path, spec_id: &str) -> String {
    let path = harness_dir.join("specs").join(spec_id).join("spec.md");
    tokio::fs::read_to_string(path).await.unwrap_or_default()
}

/// Spawn a real agent session for a **role gate** (spec 013). Mirrors
/// [`spawn_qa_agent`]: not bound to a feature, uses the feature agent tool, YOLO
/// pushed so the role-agent runs unattended.
async fn spawn_role_agent(
    state: &AppState,
    harness_id: Uuid,
    workdir: &Path,
    config: &HarnessConfig,
    role: RoleKind,
) -> anyhow::Result<agentum_core::Session> {
    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| anyhow::anyhow!("local host missing"))?;
    let name = spawn_session_name(role.as_str(), harness_id);
    let flags = if config.features.agent_yolo {
        vec![agentum_executor::YOLO_MARKER.to_string()]
    } else {
        Vec::new()
    };
    let new = NewSession {
        name: name.clone(),
        workdir: workdir.to_string_lossy().into_owned(),
        tool: config.features.agent_tool.clone(),
        model: config.features.agent_model.clone(),
        flags,
        card_id: None,
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };
    let session = state
        .store
        .create_session_on_host(new, Some(LOCAL_HOST_ID))
        .await?;
    let target = agentum_tmux::target_for(&session.name);
    crate::routes::sessions::spawn_agent_into_pane(state, &session, &host, &target, workdir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn role agent: {e}"))?;
    state
        .harness
        .set_current_session(harness_id, session.id, &session.tool, None)
        .await?;
    info!(%harness_id, role = %role.as_str(), session = %session.id, "harness spawned role agent");
    Ok(session)
}

/// Run one agent-played role gate (spec 013): spawn the role-agent with its brief
/// and the spec, wait for it to settle, read its verdict file, and decide
/// advance/retry/block via [`decide_gate`]. Fully autonomous — no human prompt
/// unless `hitl_on_block` is on AND retries are exhausted. A missing/garbled
/// verdict is a **fail** (an inconclusive gate never advances). Returns `true`
/// when the gate passed, `false` when the run halted (blocked / awaiting human).
async fn run_role_gate(
    state: &AppState,
    harness_id: Uuid,
    workdir: &Path,
    phase: SpecPhase,
) -> anyhow::Result<bool> {
    let engine = &state.harness;
    let Some(role) = phase.role() else {
        return Ok(true); // a non-agent phase (e.g. Decompose) — nothing to gate
    };

    loop {
        let config = HarnessConfig::load(workdir).await?;
        let spec_id = config
            .features
            .spec_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("role gate requires features.spec_id"))?;
        let grace = Duration::from_secs(config.features.settle_grace_secs);
        let timeout = Duration::from_secs(config.features.settle_timeout_secs);
        let spec_md = read_spec_md(&config.harness_dir, &spec_id).await;

        // Verdict file under <harness_dir>/roles/<phase>.json; clear any stale one
        // first so we never read a previous attempt's result.
        let verdict_abs = role_verdict_path(&config.harness_dir, phase);
        if let Some(parent) = verdict_abs.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::remove_file(&verdict_abs).await.ok();
        let verdict_rel = verdict_abs
            .strip_prefix(workdir)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| verdict_abs.to_string_lossy().into_owned());

        engine.log(
            harness_id,
            None,
            format!("{} gate: spawning {} agent", phase.slug(), role.as_str()),
        );
        let session = spawn_role_agent(state, harness_id, workdir, &config, role).await?;
        let prompt = build_role_prompt(
            role,
            &config.agent_instructions,
            &spec_id,
            &spec_md,
            &verdict_rel,
        );
        if !inject_prompt(state, &session, &prompt).await? {
            engine.log(harness_id, None, repl_not_ready_message());
        }
        engine.log(
            harness_id,
            None,
            format!("{} agent working…", role.as_str()),
        );
        if wait_for_settle(&state.bus, session.id, grace, timeout).await == SettleOutcome::TimedOut
        {
            engine.log(harness_id, None, settle_timeout_message(timeout));
        }
        let (passed, summary) = match tokio::fs::read_to_string(&verdict_abs).await {
            Ok(raw) => match parse_role_verdict(&raw) {
                Ok(v) => v,
                Err(e) => (
                    false,
                    format!("role verdict unreadable — failing the gate: {e}"),
                ),
            },
            Err(_) => (
                false,
                format!(
                    "{} agent wrote no verdict at {verdict_rel} — gate failed (inconclusive)",
                    role.as_str()
                ),
            ),
        };

        let attempt = engine.bump_phase_attempt(harness_id).await?;
        engine.set_gate_summary(harness_id, summary.clone()).await?;
        engine.emit(HarnessEvent::GateResult {
            harness_id,
            role,
            passed,
            attempt,
            summary: summary.clone(),
        });

        match decide_gate(
            passed,
            attempt,
            config.features.max_retries,
            config.features.hitl_on_block,
        ) {
            GateDecision::Advance => {
                teardown_session(state, &session).await;
                engine.clear_current(harness_id).await;
                let _ = append_decision(
                    workdir,
                    &format!("{} gate PASS (attempt {attempt}): {summary}", phase.slug()),
                )
                .await;
                engine.log(
                    harness_id,
                    None,
                    format!("{} gate PASS: {summary}", role.as_str()),
                );
                return Ok(true);
            }
            GateDecision::Retry => {
                teardown_session(state, &session).await;
                engine.clear_current(harness_id).await;
                engine.log(
                    harness_id,
                    None,
                    format!(
                        "{} gate CONCERNS (attempt {attempt}/{}) — retrying: {summary}",
                        role.as_str(),
                        config.features.max_retries
                    ),
                );
                continue;
            }
            GateDecision::Block => {
                let _ = append_decision(
                    workdir,
                    &format!(
                        "{} gate BLOCKED after {attempt} attempts: {summary}",
                        phase.slug()
                    ),
                )
                .await;
                engine.set_phase(harness_id, SpecPhase::Blocked).await?;
                engine.set_state(harness_id, HarnessState::Blocked).await?;
                engine.log(
                    harness_id,
                    None,
                    format!(
                        "{} gate blocked after {attempt} attempts: {summary}",
                        role.as_str()
                    ),
                );
                return Ok(false);
            }
            GateDecision::AwaitConfirm => {
                let _ = append_decision(
                    workdir,
                    &format!(
                        "{} gate AWAITING HUMAN after {attempt} attempts: {summary}",
                        phase.slug()
                    ),
                )
                .await;
                engine
                    .set_phase(harness_id, SpecPhase::AwaitingConfirm)
                    .await?;
                engine
                    .set_state(harness_id, HarnessState::AwaitingConfirmation)
                    .await?;
                engine.log(
                    harness_id,
                    None,
                    format!(
                        "{} gate awaiting human confirmation: {summary}",
                        role.as_str()
                    ),
                );
                return Ok(false);
            }
        }
    }
}

/// Run the SDD phases BEFORE the feature loop (spec 013): PM authoring gate →
/// architect gate → agentless decompose. On a blocked/parked gate it leaves the
/// run halted and returns early (the caller checks the phase before proceeding).
async fn run_pre_feature_phases(
    state: &AppState,
    harness_id: Uuid,
    workdir: &Path,
) -> anyhow::Result<()> {
    let engine = &state.harness;

    engine.set_phase(harness_id, SpecPhase::Authoring).await?;
    if !run_role_gate(state, harness_id, workdir, SpecPhase::Authoring).await? {
        return Ok(());
    }
    engine
        .set_phase(harness_id, SpecPhase::Architecture)
        .await?;
    if !run_role_gate(state, harness_id, workdir, SpecPhase::Architecture).await? {
        return Ok(());
    }

    // Agentless decompose: derive the verify-gated backlog from the (now
    // PM-refined) spec, re-apply the run knobs the fresh list would reset, then
    // make it visible to the feature loop.
    engine.set_phase(harness_id, SpecPhase::Decompose).await?;
    let config = HarnessConfig::load(workdir).await?;
    let spec_id = config
        .features
        .spec_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("decompose requires features.spec_id"))?;
    engine.log(
        harness_id,
        None,
        format!("decompose: deriving backlog from spec {spec_id}"),
    );
    // Decompose must not drop tracker provenance (spec 006 C1): the fresh
    // backlog's features need the issue's provider/url re-stamped, or every
    // later `transition_tracker` call silently no-ops and the status-label
    // trail dies the moment roles are on. `copy_knobs_from` below copies only
    // list-level knobs, never per-feature stamps.
    let mut derived = match shared_tracker_provenance(&config.features) {
        Some((provider, url)) => {
            plan_from_spec_with_tracker(workdir, &spec_id, &provider, &url).await?
        }
        None => plan_from_spec(workdir, &spec_id).await?,
    };
    derived.copy_knobs_from(&config.features);
    config.save_features(&derived).await?;
    engine.reload_features(harness_id).await?;
    engine.set_phase(harness_id, SpecPhase::Executing).await?;
    Ok(())
}

/// Run the reviewer gate AFTER all features are green (spec 013). On pass the
/// phase advances to `Done`; a blocked/parked gate leaves it halted.
async fn run_review_phase(
    state: &AppState,
    harness_id: Uuid,
    workdir: &Path,
) -> anyhow::Result<()> {
    let engine = &state.harness;
    engine.set_phase(harness_id, SpecPhase::Review).await?;
    if run_role_gate(state, harness_id, workdir, SpecPhase::Review).await? {
        engine.set_phase(harness_id, SpecPhase::Done).await?;
    }
    Ok(())
}

/// How long to wait after typing a multi-line prompt before sending the
/// submitting Enter. A TUI like Claude Code coalesces a fast multi-line burst
/// into a single bracketed-paste block; the Enter must arrive as its own
/// keystroke *after* that, or it gets swallowed into the paste and the prompt
/// just sits unsent in the input box.
const SUBMIT_DELAY: Duration = Duration::from_millis(600);

/// Wait until the agent's REPL is ready to accept a prompt, transparently
/// accepting Claude's one-time workspace-trust dialog along the way.
///
/// Two things make a fixed boot delay too fragile to type after. First, on a
/// fresh workdir Claude shows "Do you trust this folder?" — NOT skipped by
/// `--dangerously-skip-permissions` (only by non-interactive `-p` mode) — and
/// typing the prompt while that dialog is up feeds the task text into the menu
/// and loses it. Second, with MCP servers wired the boot can take 10–30s+, so
/// the dialog appears late and unpredictably.
///
/// So we poll the pane instead: accept the trust dialog the instant it appears,
/// and return once the idle input footer is visible. Bounded (~56s); a remote
/// pane or unrecognised tool falls back to a fixed delay so it still gets typed.
///
/// Returns `true` when the idle REPL footer was actually seen (ready confirmed)
/// and `false` when it fell through — a remote fixed-delay fallback, a missing
/// host, or the ~56 s poll expiring without the footer. `false` means the prompt
/// is about to fire BLIND; the caller surfaces that loudly (spec 008 F1 #14a).
/// The poll / trust-accept / fixed-delay logic is otherwise byte-for-byte the
/// pre-008 behavior — only the return type changed (D5 sacred-mechanic gate).
async fn await_repl_ready(state: &AppState, session: &agentum_core::Session) -> bool {
    let host = match state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await
    {
        Ok(Some(h)) => h,
        _ => return false,
    };
    // We can only cheaply capture local panes; remote panes get a fixed delay.
    if !matches!(host.kind, HostKind::Local) {
        tokio::time::sleep(AGENT_BOOT_DELAY).await;
        return false;
    }
    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));

    let mut trusted = false;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let pane = match crate::host_runtime::capture_pane_visible(&host, &target).await {
            Ok(p) => p.to_lowercase(),
            Err(_) => continue,
        };
        if !trusted && (pane.contains("trust this folder") || pane.contains("do you trust")) {
            // Confirm the pre-selected "Yes, I trust this folder" (Enter), then
            // keep polling until the REPL itself is up.
            let _ = crate::host_runtime::send_keys(&host, &target, "", true).await;
            trusted = true;
            tokio::time::sleep(Duration::from_millis(900)).await;
            continue;
        }
        // Idle REPL footer → ready for input. Covers YOLO ("bypass permissions
        // on" / "shift+tab to cycle") and default ("? for shortcuts") modes.
        if pane.contains("bypass permissions on")
            || pane.contains("shift+tab to cycle")
            || pane.contains("? for shortcuts")
        {
            // A beat to ensure the input is focused before we type.
            tokio::time::sleep(Duration::from_millis(400)).await;
            return true;
        }
    }
    // ~56 s elapsed without ever seeing the idle footer — the prompt will be
    // typed blind. Report it so the caller can warn (spec 008 F1 #14a).
    false
}

/// Hand the agent a prompt: wait for the REPL to be ready (accepting the trust
/// dialog, outlasting an MCP-slowed boot), then type it and submit.
///
/// The submit is a deliberate **two-step**: type the prompt with NO trailing
/// Enter, pause, then send a bare Enter. A single combined
/// `send-keys "<text>" Enter` is swallowed by the REPL's paste handling for a
/// multi-line prompt — the text lands in the input box (often collapsed to a
/// "[Pasted text]" block) but never executes. The separate, delayed Enter is
/// what actually runs the agent's turn.
///
/// `pub(crate)` so the board-goals planner/card spawns reuse the exact same
/// robust delivery (they previously used a one-shot `send_keys(prompt, true)`
/// that the REPL swallowed — the chat then sat at "Drafting cards…" forever).
///
/// Returns whether the REPL was CONFIRMED ready before typing (bubbled from
/// [`await_repl_ready`]) — `false` means the prompt fired blind. The send
/// sequence below (`send_bytes` → `SUBMIT_DELAY` → bare Enter) is byte-for-byte
/// unchanged; only the return type carries the readiness bool through (spec 008
/// F1 #14a). Callers that don't care (`board_goals`/`sessions`/`wiki`) match on
/// `Err` and ignore the `Ok(bool)`.
pub(crate) async fn inject_prompt(
    state: &AppState,
    session: &agentum_core::Session,
    prompt: &str,
) -> anyhow::Result<bool> {
    let ready = await_repl_ready(state, session).await;
    let host = state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await?
        .ok_or_else(|| anyhow::anyhow!("session host missing"))?;
    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
    // Step 1: paste the prompt body, no Enter. Deliver as chunked raw bytes
    // (`send-keys -H`, the same path the interactive terminal uses) rather than
    // one `tmux send-keys "<text>"`: a large prompt — the AutoWiki generator
    // inlines a ~22k-token repo-context starter map — overflows tmux's command
    // length and fails the whole run with "command too long". `send_bytes`
    // chunks under the argv limit, so prompt size stops being a cliff.
    crate::host_runtime::send_bytes(&host, &target, prompt.as_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("send prompt failed: {e}"))?;
    // Step 2: let the paste settle, then submit with a bare Enter.
    tokio::time::sleep(SUBMIT_DELAY).await;
    crate::host_runtime::send_keys(&host, &target, "", true)
        .await
        .map_err(|e| anyhow::anyhow!("submit Enter failed: {e}"))?;
    Ok(ready)
}

/// Gracefully mark an agent stopped + retire its pane. Best-effort.
pub(crate) async fn teardown_session(state: &AppState, session: &agentum_core::Session) {
    let host = match state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await
    {
        Ok(Some(h)) => h,
        _ => return,
    };
    if matches!(host.kind, HostKind::Local | HostKind::Ssh { .. }) {
        let target = session
            .tmux_target
            .clone()
            .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
        // Persist first: the watchdog can observe the pane disappear between
        // kill and the following DB write. The old order misclassified normal
        // harness teardown as `session.crashed`, which could strand recovery
        // surfaces on a dead "current" agent.
        let _ = state
            .store
            .update_status_and_target(session.id, Status::Stopped, None)
            .await;
        let _ = crate::host_runtime::kill_session(&host, &target).await;
    }
}

/// How a [`wait_for_settle`] wait ended. The whole point of returning this (vs.
/// the old silent `()`) is so the caller can make `TimedOut` LOUD (spec 008 F1
/// #15): before this, an agent that never signalled idle let the gate run on a
/// possibly-unchanged tree after up to `settle_timeout_secs` (default 1800 s)
/// with zero events — a silent hang. `Settled`/`Crashed` are the quiet, normal
/// endings; only `TimedOut` warrants a warning at the drive call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettleOutcome {
    /// The agent went idle (`agent.awaiting_input`/`agent.finished`), or the
    /// event bus closed (shutdown) — proceed to the gate as before.
    Settled,
    /// The session crashed or was stopped mid-turn.
    Crashed,
    /// The settle window elapsed with no idle signal — the agent may be stuck or
    /// the prompt never landed. The gate still runs, but loudly.
    TimedOut,
}

/// The loud warning the drive loop emits when a settle times out (spec 008 F1
/// #15). Kept as one builder so every call site phrases the 1800 s silent-hang
/// closure identically.
fn settle_timeout_message(timeout: Duration) -> String {
    format!(
        "⚠ no settle signal in {}s — the agent may be stuck or the prompt didn't land; running the gate anyway",
        timeout.as_secs()
    )
}

/// The loud warning emitted when [`inject_prompt`] reports the REPL never
/// signalled ready (spec 008 F1 #14a): the prompt was typed blind, so if the
/// pane stays empty it may not have landed. One builder so every call site says
/// it the same way.
fn repl_not_ready_message() -> &'static str {
    "⚠ agent REPL never signalled ready in ~56 s — prompt sent anyway; if the pane shows no output the prompt may not have landed"
}

/// Wait until the agent in `session_id` looks done with its turn — the first
/// `agent.awaiting_input` / `agent.finished` event after `grace` — or until
/// `timeout` elapses (then we run the gate anyway). A crash/stop also returns.
/// Returns [`SettleOutcome`] so the caller can surface a `TimedOut` loudly
/// instead of gating in silence (spec 008 F1 #15).
pub(crate) async fn wait_for_settle(
    bus: &broadcast::Sender<Event>,
    session_id: Uuid,
    grace: Duration,
    timeout: Duration,
) -> SettleOutcome {
    let mut rx = bus.subscribe();
    let start = Instant::now();
    // Did the agent go idle *inside* the grace window? If so we don't discard
    // that signal (which would strand a fast feature waiting out the full
    // `timeout`) — we return as soon as grace elapses.
    let mut settled_early = false;
    loop {
        if settled_early && start.elapsed() >= grace {
            return SettleOutcome::Settled;
        }
        let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
            return SettleOutcome::TimedOut; // overall settle timeout — gate anyway
        };
        // While a settle is pending, cap the wait at the grace boundary so we
        // re-evaluate the early-return condition the moment grace expires.
        let wait = if settled_early {
            grace
                .checked_sub(start.elapsed())
                .unwrap_or(Duration::ZERO)
                .min(remaining)
        } else {
            remaining
        };
        match tokio::time::timeout(wait, rx.recv()).await {
            Err(_) => {
                // Either we hit the grace boundary (loop re-checks settled_early)
                // or the overall settle timeout elapsed.
                if start.elapsed() >= timeout {
                    return SettleOutcome::TimedOut;
                }
                continue;
            }
            Ok(Ok(ev)) => {
                if ev.session_id != Some(session_id) {
                    continue;
                }
                match ev.kind.as_str() {
                    "agent.awaiting_input" | "agent.finished" => {
                        // The watchdog's FIRST classification of a fresh pane
                        // arrives as one of these kinds with {"initial": true} —
                        // the REPL booting to its idle prompt, not the agent
                        // finishing the injected turn. Counting it as a settle
                        // tears a just-started agent down at the grace boundary
                        // (#302), so boot-time classifications never settle.
                        if ev.payload.get("initial").and_then(|v| v.as_bool()) == Some(true) {
                            continue;
                        }
                        if start.elapsed() >= grace {
                            return SettleOutcome::Settled;
                        }
                        // The agent's *initial* idle, before grace — remember it
                        // and return once the grace window closes.
                        settled_early = true;
                    }
                    "session.crashed" | "session.stopped" => return SettleOutcome::Crashed,
                    _ => {}
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            // The bus closed (shutdown) — not a timeout; proceed quietly as the
            // old `()` return did, without a spurious loud warning.
            Ok(Err(broadcast::error::RecvError::Closed)) => return SettleOutcome::Settled,
        }
    }
}
