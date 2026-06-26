//! Harness Engine — orchestrates real agent execution behind a verification gate.
//!
//! A "harness" is a project directory containing a `.harness/` folder:
//! - `AGENTS.md`         — instructions handed to every agent as scope/context
//! - `feature_list.json` — the ordered feature backlog + per-feature state
//! - `init.sh`           — environment smoke-test, run once before any feature
//! - `verify.sh`         — the gate: run after each feature; non-zero = blocked
//! - `handoff.md`        — written after each feature so the next session has state
//!
//! The engine drives one feature at a time: it spawns a REAL agent session
//! (Claude/Codex/… in a tmux pane via the same launch path the `start` route
//! uses), scopes it to the current feature, waits for the agent to settle, then
//! runs `verify.sh`. **A red verify BLOCKS advancement** — the feature is
//! retried (the agent is handed the failure output) until it passes or hits
//! `max_retries`, at which point the whole run halts in `Blocked`. A green
//! verify marks the feature `done`, writes `handoff.md`, and advances.
//!
//! The state machine itself ([`HarnessEngine`]) is decoupled from spawning so it
//! can be unit-tested with stub `verify.sh` scripts; the live orchestration that
//! actually launches agents lives in [`drive`], which takes the full
//! [`AppState`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentum_core::{Event, HostKind, LOCAL_HOST_ID, NewSession, Status};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;

// Harness data types + on-disk `.agentum-harness/` operations live in `types`;
// the engine, drive loop, and gate helpers below still reference them directly
// via this glob re-export (which also preserves the `harness::Foo` public API).
mod types;
pub use types::*;

// Prompt builders + verdict parsers + small utilities; `pub(crate)` items the
// drive/gate code below calls. Internal (not re-exported in the public surface).
mod helpers;
use helpers::*;

/// Manages every concurrent harness run + the event bus they publish on.
pub struct HarnessEngine {
    runs: RwLock<HashMap<Uuid, Arc<RwLock<HarnessRun>>>>,
    event_tx: broadcast::Sender<HarnessEvent>,
}

impl HarnessEngine {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(512);
        Self {
            runs: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    /// Subscribe to the harness event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<HarnessEvent> {
        self.event_tx.subscribe()
    }

    /// Register a new run from a project directory. Loads `.harness/` so a bad
    /// config fails fast (before the UI shows a run that can never start).
    pub async fn start(&self, workdir: PathBuf) -> anyhow::Result<Uuid> {
        let config = HarnessConfig::load(&workdir).await?;
        let id = Uuid::new_v4();

        // Restore the SDD phase from the durable decision log so a re-registered
        // run (store wipe, daemon restart) resumes where it left off (spec 013).
        // Defaults to `Executing` for a fresh or pre-013 run.
        let phase = rebuild_phase_from_decisions(&read_decisions(&workdir).await)
            .unwrap_or(SpecPhase::Executing);

        let run = HarnessRun {
            id,
            workdir,
            state: HarnessState::Idle,
            features: config.features.clone(),
            current_feature: None,
            current_session: None,
            started_at: Instant::now(),
            agent_instructions: config.agent_instructions.clone(),
            driving: false,
            phase,
            phase_attempts: 0,
        };

        self.runs
            .write()
            .await
            .insert(id, Arc::new(RwLock::new(run)));

        self.emit(HarnessEvent::StateChanged {
            harness_id: id,
            state: HarnessState::Idle,
        });
        Ok(id)
    }

    /// Run `init.sh` (the environment smoke-test). Returns `false` (and sets the
    /// run `Failed`) on a non-zero exit. A missing `init.sh` is a pass.
    pub async fn run_init(&self, harness_id: Uuid) -> anyhow::Result<bool> {
        let workdir = self.workdir(harness_id).await?;
        self.set_state(harness_id, HarnessState::InitVerifying)
            .await?;
        self.emit(HarnessEvent::InitStarted { harness_id });

        let config = HarnessConfig::load(&workdir).await?;
        let (success, output) = match config.init_script {
            Some(script) => {
                let out = Command::new("bash")
                    .arg(&script)
                    .current_dir(&workdir)
                    .output()
                    .await?;
                (
                    out.status.success(),
                    combine_output(&out.stdout, &out.stderr),
                )
            }
            None => (true, "no init.sh — skipping environment check".to_string()),
        };

        self.emit(HarnessEvent::InitCompleted {
            harness_id,
            success,
            output,
        });
        if !success {
            self.set_state(harness_id, HarnessState::Failed).await?;
        } else {
            // Don't strand the run in `InitVerifying` after a standalone
            // (manual `POST /{id}/init`) check passes. The driver immediately
            // overrides this with `Running`, so the extra transition is only
            // observable for the manual path, where `Idle` is the correct
            // resting state.
            self.set_state(harness_id, HarnessState::Idle).await?;
        }
        Ok(success)
    }

    /// Run `verify.sh` for a feature WITHOUT finalizing its state. Returns
    /// `(passed, output)`. Falls back to `npm run verify`, then to a pass when
    /// no verification is configured. The driver owns the done/blocked decision
    /// so it can implement the retry loop; the one-shot HTTP endpoint composes
    /// this with [`Self::mark_feature_done`] / [`Self::record_feature_failure`].
    pub async fn run_verify_once(
        &self,
        harness_id: Uuid,
        feature_id: &str,
    ) -> anyhow::Result<(bool, String)> {
        let workdir = self.workdir(harness_id).await?;
        self.set_feature_state(harness_id, feature_id, FeatureState::Verifying)
            .await?;
        self.set_state(harness_id, HarnessState::Verifying).await?;
        self.emit(HarnessEvent::VerifyStarted {
            harness_id,
            feature_id: feature_id.to_string(),
        });

        let config = HarnessConfig::load(&workdir).await?;
        let (success, output) = if let Some(script) = config.verify_script {
            let out = Command::new("bash")
                .arg(&script)
                .env("HARNESS_FEATURE_ID", feature_id)
                .current_dir(&workdir)
                .output()
                .await?;
            (
                out.status.success(),
                combine_output(&out.stdout, &out.stderr),
            )
        } else {
            match Command::new("npm")
                .args(["run", "verify"])
                .current_dir(&workdir)
                .output()
                .await
            {
                Ok(out) => (
                    out.status.success(),
                    combine_output(&out.stdout, &out.stderr),
                ),
                // No verify.sh and no `npm run verify`: nothing to gate on.
                Err(_) => (
                    true,
                    "no verify.sh and no `npm run verify` — gate skipped".to_string(),
                ),
            }
        };

        self.emit(HarnessEvent::VerifyCompleted {
            harness_id,
            feature_id: feature_id.to_string(),
            success,
            output: output.clone(),
        });
        Ok((success, output))
    }

    /// Run the **browser QA gate** (`qa.sh`) for a feature WITHOUT finalizing its
    /// state. Flips the feature to `ReadyToTest` first (the tracker's "Ready to
    /// Test" column maps to this), then runs `qa.sh` with `$HARNESS_FEATURE_ID`
    /// set. A missing `qa.sh` is a **pass** — a non-web project isn't blocked on a
    /// browser check; web projects scaffold a `qa.sh` that runs the
    /// browser-verification-loop (spec 012). The driver owns the done/retry
    /// decision, exactly like [`Self::run_verify_once`].
    pub async fn run_qa_once(
        &self,
        harness_id: Uuid,
        feature_id: &str,
    ) -> anyhow::Result<(bool, String)> {
        let workdir = self.workdir(harness_id).await?;
        self.set_feature_state(harness_id, feature_id, FeatureState::ReadyToTest)
            .await?;
        self.log(
            harness_id,
            Some(feature_id),
            "unit gate green — running browser QA gate (qa.sh)",
        );

        let config = HarnessConfig::load(&workdir).await?;
        let (success, output) = if let Some(script) = config.qa_script {
            let out = Command::new("bash")
                .arg(&script)
                .env("HARNESS_FEATURE_ID", feature_id)
                .current_dir(&workdir)
                .output()
                .await?;
            (
                out.status.success(),
                combine_output(&out.stdout, &out.stderr),
            )
        } else {
            (
                true,
                "no qa.sh — browser QA gate skipped (no web surface to verify)".to_string(),
            )
        };
        Ok((success, output))
    }

    /// Mark a feature `Done`, persist, and write `handoff.md`.
    pub async fn mark_feature_done(
        &self,
        harness_id: Uuid,
        feature_id: &str,
        output: &str,
    ) -> anyhow::Result<()> {
        self.set_feature_state(harness_id, feature_id, FeatureState::Done)
            .await?;

        let (workdir, feature) = {
            let run = self.get_run(harness_id).await?;
            let r = run.read().await;
            (
                r.workdir.clone(),
                r.features
                    .features
                    .iter()
                    .find(|f| f.id == feature_id)
                    .cloned(),
            )
        };
        if let Some(feature) = feature {
            let config = HarnessConfig::load(&workdir).await?;
            config.write_handoff(&feature, output).await?;
            self.emit(HarnessEvent::HandoffWritten {
                harness_id,
                feature_id: feature_id.to_string(),
            });
        }
        Ok(())
    }

    /// Whether this run requires one human confirmation at the QA gate (HITL-at-QA, 010c).
    pub async fn hitl_at_qa(&self, harness_id: Uuid) -> anyhow::Result<bool> {
        let run = self.get_run(harness_id).await?;
        let r = run.read().await;
        Ok(r.features.hitl_at_qa)
    }

    /// Verify passed but HITL-at-QA is on: park the feature in `AwaitingConfirm`
    /// and pause the run. Persisted so the board shows the pending confirmation.
    pub async fn await_confirm(&self, harness_id: Uuid, feature_id: &str) -> anyhow::Result<()> {
        self.set_feature_state(harness_id, feature_id, FeatureState::AwaitingConfirm)
            .await?;
        self.set_state(harness_id, HarnessState::AwaitingConfirmation)
            .await?;
        Ok(())
    }

    /// Human confirms a feature parked in `AwaitingConfirm` → finalize it `Done`
    /// (writes `handoff.md`). Errors if the feature isn't awaiting confirmation,
    /// so a stray confirm can't fast-track an unverified feature.
    pub async fn confirm_feature(&self, harness_id: Uuid, feature_id: &str) -> anyhow::Result<()> {
        let state = {
            let run = self.get_run(harness_id).await?;
            let r = run.read().await;
            r.features
                .features
                .iter()
                .find(|f| f.id == feature_id)
                .map(|f| f.state)
        };
        match state {
            Some(FeatureState::AwaitingConfirm) => {}
            Some(other) => {
                anyhow::bail!("feature {feature_id} is {other:?}, not awaiting confirmation")
            }
            None => anyhow::bail!("no such feature: {feature_id}"),
        }
        self.mark_feature_done(
            harness_id,
            feature_id,
            "Human-confirmed at the QA gate (HITL-at-QA).",
        )
        .await
    }

    /// Record a verify failure: bump `attempts`, store the error. Returns
    /// `true` when the feature has now exhausted `max_retries` and is `Blocked`;
    /// otherwise the feature is left `Coding` for another attempt.
    pub async fn record_feature_failure(
        &self,
        harness_id: Uuid,
        feature_id: &str,
        output: &str,
    ) -> anyhow::Result<bool> {
        let run = self.get_run(harness_id).await?;
        let (blocked, workdir, features_snapshot) = {
            let mut r = run.write().await;
            let max_retries = r.features.max_retries;
            let mut blocked = false;
            if let Some(feature) = r.features.features.iter_mut().find(|f| f.id == feature_id) {
                feature.attempts += 1;
                feature.last_error = Some(tail(output, 4000));
                if feature.attempts >= max_retries {
                    feature.state = FeatureState::Blocked;
                    blocked = true;
                } else {
                    feature.state = FeatureState::Coding;
                }
            }
            (blocked, r.workdir.clone(), r.features.clone())
        };

        // Persist outside the lock-held mutation above.
        let config = HarnessConfig::load(&workdir).await?;
        config.save_features(&features_snapshot).await?;

        let new_state = if blocked {
            FeatureState::Blocked
        } else {
            FeatureState::Coding
        };
        self.emit(HarnessEvent::FeatureStateChanged {
            harness_id,
            feature_id: feature_id.to_string(),
            state: new_state,
        });
        Ok(blocked)
    }

    /// One-shot verify used by the manual `POST /{id}/verify` endpoint: run the
    /// gate and finalize the feature (done on green, attempt-counted on red).
    pub async fn run_verify(&self, harness_id: Uuid, feature_id: &str) -> anyhow::Result<bool> {
        let (success, output) = self.run_verify_once(harness_id, feature_id).await?;
        if success {
            if self.hitl_at_qa(harness_id).await? {
                // Park for human confirmation instead of locking in (HITL-at-QA).
                self.await_confirm(harness_id, feature_id).await?;
            } else {
                self.mark_feature_done(harness_id, feature_id, &output)
                    .await?;
            }
        } else {
            self.record_feature_failure(harness_id, feature_id, &output)
                .await?;
        }
        Ok(success)
    }

    /// The next `Pending` feature in backlog order, if any.
    pub async fn next_pending_feature(&self, harness_id: Uuid) -> anyhow::Result<Option<Feature>> {
        let run = self.get_run(harness_id).await?;
        let r = run.read().await;
        Ok(r.features
            .features
            .iter()
            .find(|f| f.state == FeatureState::Pending)
            .cloned())
    }

    /// Full status snapshot for the UI.
    pub async fn status(&self, harness_id: Uuid) -> anyhow::Result<HarnessStatus> {
        let run = self.get_run(harness_id).await?;
        let r = run.read().await;
        Ok(HarnessStatus {
            id: r.id,
            workdir: r.workdir.to_string_lossy().into_owned(),
            state: r.state,
            features: r.features.clone(),
            current_feature: r.current_feature.clone(),
            current_session: r.current_session,
            elapsed_secs: r.started_at.elapsed().as_secs(),
            agent_instructions: r.agent_instructions.clone(),
            phase: r.phase,
            phase_attempts: r.phase_attempts,
        })
    }

    /// All registered run ids.
    pub async fn list(&self) -> Vec<Uuid> {
        self.runs.read().await.keys().copied().collect()
    }

    /// The project workdir for a run.
    pub async fn workdir(&self, harness_id: Uuid) -> anyhow::Result<PathBuf> {
        let run = self.get_run(harness_id).await?;
        let r = run.read().await;
        Ok(r.workdir.clone())
    }

    /// Atomically claim the driver slot. Returns `false` if a drive loop is
    /// already live so the route can reject a double-run instead of spawning two
    /// loops. The slot is freed by [`Self::release_driver`] when the loop exits
    /// (done/blocked/failed/stopped), so a finished run can always be re-driven.
    pub async fn claim_driver(&self, harness_id: Uuid) -> anyhow::Result<bool> {
        let run = self.get_run(harness_id).await?;
        let mut r = run.write().await;
        if r.driving {
            return Ok(false);
        }
        r.driving = true;
        Ok(true)
    }

    /// Free the driver slot. Best-effort: a run removed mid-drive (user pressed
    /// Stop) is a no-op. Always called once [`drive`] returns, regardless of how.
    pub async fn release_driver(&self, harness_id: Uuid) {
        if let Ok(run) = self.get_run(harness_id).await {
            run.write().await.driving = false;
        }
    }

    /// Reset every `Blocked` feature back to `Pending` with a fresh retry budget.
    /// A blocked feature halts the run; on a re-run we want to *retry* it, not
    /// silently skip past it to the next pending feature (`next_pending_feature`
    /// ignores `Blocked`). Returns how many were reset.
    pub async fn reset_blocked_features(&self, harness_id: Uuid) -> anyhow::Result<usize> {
        let (reset_ids, workdir, snapshot) = {
            let run = self.get_run(harness_id).await?;
            let mut r = run.write().await;
            let mut reset_ids = Vec::new();
            for f in r.features.features.iter_mut() {
                if f.state == FeatureState::Blocked {
                    f.state = FeatureState::Pending;
                    f.attempts = 0;
                    f.last_error = None;
                    reset_ids.push(f.id.clone());
                }
            }
            (reset_ids, r.workdir.clone(), r.features.clone())
        };
        if !reset_ids.is_empty() {
            let config = HarnessConfig::load(&workdir).await?;
            config.save_features(&snapshot).await?;
            for id in &reset_ids {
                self.emit(HarnessEvent::FeatureStateChanged {
                    harness_id,
                    feature_id: id.clone(),
                    state: FeatureState::Pending,
                });
            }
        }
        Ok(reset_ids.len())
    }

    /// Clear the "current feature/session" pointers once the run has no more
    /// work, so the UI doesn't keep pinning the last feature as active.
    pub async fn clear_current(&self, harness_id: Uuid) {
        if let Ok(run) = self.get_run(harness_id).await {
            let mut r = run.write().await;
            r.current_feature = None;
            r.current_session = None;
        }
    }

    /// Drop a run from the map. Best-effort; the agent's tmux pane (if any) is
    /// left to the caller / watchdog.
    pub async fn stop(&self, harness_id: Uuid) -> anyhow::Result<()> {
        self.runs.write().await.remove(&harness_id);
        self.emit(HarnessEvent::HarnessCompleted {
            harness_id,
            success: false,
        });
        Ok(())
    }

    /// Bind the current agent session to a feature and flip the feature to
    /// `Coding`. Emits `AgentSpawned`.
    pub async fn set_session(
        &self,
        harness_id: Uuid,
        session_id: Uuid,
        feature_id: &str,
    ) -> anyhow::Result<()> {
        {
            let run = self.get_run(harness_id).await?;
            let mut r = run.write().await;
            r.current_session = Some(session_id);
            r.current_feature = Some(feature_id.to_string());
        }
        self.set_feature_state(harness_id, feature_id, FeatureState::Coding)
            .await?;
        self.emit(HarnessEvent::AgentSpawned {
            harness_id,
            feature_id: feature_id.to_string(),
            session_id,
        });
        Ok(())
    }

    /// Set the overall run state + emit.
    pub async fn set_state(&self, harness_id: Uuid, state: HarnessState) -> anyhow::Result<()> {
        {
            let run = self.get_run(harness_id).await?;
            run.write().await.state = state;
        }
        self.emit(HarnessEvent::StateChanged { harness_id, state });
        Ok(())
    }

    /// Emit a human-readable progress line into the event stream.
    pub fn log(&self, harness_id: Uuid, feature_id: Option<&str>, message: impl Into<String>) {
        self.emit(HarnessEvent::Log {
            harness_id,
            feature_id: feature_id.map(|s| s.to_string()),
            message: message.into(),
        });
    }

    pub fn emit_error(&self, harness_id: Uuid, message: impl Into<String>) {
        self.emit(HarnessEvent::Error {
            harness_id,
            message: message.into(),
        });
    }

    async fn get_run(&self, harness_id: Uuid) -> anyhow::Result<Arc<RwLock<HarnessRun>>> {
        self.runs
            .read()
            .await
            .get(&harness_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("harness {harness_id} not found"))
    }

    async fn set_feature_state(
        &self,
        harness_id: Uuid,
        feature_id: &str,
        state: FeatureState,
    ) -> anyhow::Result<()> {
        let (workdir, features_snapshot) = {
            let run = self.get_run(harness_id).await?;
            let mut r = run.write().await;
            if let Some(feature) = r.features.features.iter_mut().find(|f| f.id == feature_id) {
                feature.state = state;
            }
            (r.workdir.clone(), r.features.clone())
        };
        // Persist the board to disk so the on-disk feature_list.json stays the
        // single source of truth even mid-run.
        let config = HarnessConfig::load(&workdir).await?;
        config.save_features(&features_snapshot).await?;

        self.emit(HarnessEvent::FeatureStateChanged {
            harness_id,
            feature_id: feature_id.to_string(),
            state,
        });
        Ok(())
    }

    /// Advance the run to a new SDD phase (spec 013): update in-memory state,
    /// reset the gate attempt counter, append a durable marker to `decisions.md`
    /// (the canonical record [`rebuild_phase_from_decisions`] reads on rescan),
    /// and emit `PhaseChanged`.
    async fn set_phase(&self, harness_id: Uuid, to: SpecPhase) -> anyhow::Result<()> {
        let (workdir, from) = {
            let run = self.get_run(harness_id).await?;
            let mut r = run.write().await;
            let from = r.phase;
            r.phase = to;
            r.phase_attempts = 0;
            (r.workdir.clone(), from)
        };
        if from != to {
            // One canonical "entered <phase>" marker per transition.
            let _ = append_decision(
                &workdir,
                &format!("phase: entered {} (from {})", to.slug(), from.slug()),
            )
            .await;
        }
        self.emit(HarnessEvent::PhaseChanged {
            harness_id,
            from,
            to,
        });
        Ok(())
    }

    /// The run's current SDD phase.
    pub async fn phase(&self, harness_id: Uuid) -> anyhow::Result<SpecPhase> {
        let run = self.get_run(harness_id).await?;
        let p = run.read().await.phase;
        Ok(p)
    }

    /// Bump the current phase's gate attempt counter; returns the new count.
    async fn bump_phase_attempt(&self, harness_id: Uuid) -> anyhow::Result<u32> {
        let run = self.get_run(harness_id).await?;
        let mut r = run.write().await;
        r.phase_attempts += 1;
        Ok(r.phase_attempts)
    }

    /// Reload the in-memory backlog from disk after `decompose` rewrites
    /// `feature_list.json` via [`plan_from_spec`]. `next_pending_feature` reads
    /// the in-memory list, so without this the freshly-derived features are
    /// invisible to the loop.
    async fn reload_features(&self, harness_id: Uuid) -> anyhow::Result<()> {
        let workdir = self.workdir(harness_id).await?;
        let config = HarnessConfig::load(&workdir).await?;
        let run = self.get_run(harness_id).await?;
        run.write().await.features = config.features;
        Ok(())
    }

    fn emit(&self, event: HarnessEvent) {
        // Err only means no subscribers — fine, the event is best-effort.
        let _ = self.event_tx.send(event);
    }
}

impl Default for HarnessEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Status payload returned by `GET /api/harness/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessStatus {
    pub id: Uuid,
    pub workdir: String,
    pub state: HarnessState,
    pub features: FeatureList,
    pub current_feature: Option<String>,
    pub current_session: Option<Uuid>,
    pub elapsed_secs: u64,
    pub agent_instructions: String,
    /// Current SDD phase (spec 013). `executing` for a plain feature run.
    #[serde(default)]
    pub phase: SpecPhase,
    /// Role-gate retry counter for the current phase (spec 013).
    #[serde(default)]
    pub phase_attempts: u32,
}

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
        if state.harness.status(harness_id).await.is_ok() {
            warn!(%harness_id, error = %e, "harness run failed");
            state.harness.emit_error(harness_id, e.to_string());
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
        let prompt = build_feature_prompt(&config.agent_instructions, &feature);
        inject_prompt(state, &session, &prompt).await?;
        engine.log(harness_id, Some(&feature.id), "agent working…");
        wait_for_settle(&state.bus, session.id, grace, timeout).await;

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
            // Pick the QA gate: a spawned browser-verification-loop agent (012b)
            // or the `qa.sh` shell gate. `Auto` prefers qa.sh, else an agent when
            // browser-verify is on, else the skip-pass script path.
            let qa_mode = resolve_qa_mode(&config);
            let (qa_ok, qa_out) = match qa_mode {
                QaMode::Agent => {
                    run_qa_agent_gate(
                        state, harness_id, &workdir, &config, &feature, grace, timeout,
                    )
                    .await?
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
    let blocked = engine
        .record_feature_failure(harness_id, &feature.id, output)
        .await?;
    if blocked {
        engine.set_state(harness_id, HarnessState::Blocked).await?;
        engine.log(
            harness_id,
            Some(&feature.id),
            format!("✗ {gate_label} FAILED — retries exhausted, feature BLOCKED. Run halted."),
        );
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
    inject_prompt(state, session, &retry).await?;
    wait_for_settle(&state.bus, session.id, grace, timeout).await;
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
    match crate::task_sink::apply_tracker_transition(&state.store, provider, &feature.id, phase)
        .await
    {
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

    // A unique, tmux-safe session name per feature+run.
    let short = harness_id.simple().to_string();
    let name = format!("harness-{}-{}", sanitize(&feature.id), &short[..8]);

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
        .set_session(harness_id, session.id, &feature.id)
        .await?;

    let target = agentum_tmux::target_for(&session.name);
    crate::routes::sessions::spawn_agent_into_pane(state, &session, &host, &target, workdir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn agent: {e}"))?;

    info!(%harness_id, feature = %feature.id, session = %session.id, "harness spawned agent");
    Ok(session)
}

/// Resolve the configured [`QaMode`] to a concrete gate for this run. `Auto`
/// prefers an explicit `qa.sh`, else a QA agent when browser-verify is enabled,
/// else the (skip-pass) script path. Pure-ish (only reads the env flag).
fn resolve_qa_mode(config: &HarnessConfig) -> QaMode {
    match config.features.qa_mode {
        QaMode::Script => QaMode::Script,
        QaMode::Agent => QaMode::Agent,
        QaMode::Auto => {
            if config.qa_script.is_some() {
                QaMode::Script
            } else if crate::playwright_mcp::feature_enabled() {
                QaMode::Agent
            } else {
                // No qa.sh and no browser-verify → the script path returns a
                // skip-pass, so a non-web project isn't blocked.
                QaMode::Script
            }
        }
    }
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
    let short = harness_id.simple().to_string();
    let name = format!("harness-qa-{}-{}", sanitize(&feature.id), &short[..8]);
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

    if !crate::playwright_mcp::feature_enabled() {
        engine.log(
            harness_id,
            Some(&feature.id),
            "QA agent: AGENTUM_BROWSER_VERIFY is not set, so no Playwright MCP is wired — the agent may be unable to drive a browser.",
        );
    }
    engine.log(
        harness_id,
        Some(&feature.id),
        "unit gate green — spawning browser QA agent (browser-verification-loop)",
    );

    let session = spawn_qa_agent(state, harness_id, workdir, config, feature).await?;
    let prompt = build_qa_prompt(&config.agent_instructions, feature, &verdict_rel);
    inject_prompt(state, &session, &prompt).await?;
    engine.log(
        harness_id,
        Some(&feature.id),
        "QA agent verifying in browser…",
    );
    wait_for_settle(&state.bus, session.id, grace, timeout).await;
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
    let short = harness_id.simple().to_string();
    let name = format!("harness-{}-{}", role.as_str(), &short[..8]);
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
        inject_prompt(state, &session, &prompt).await?;
        engine.log(
            harness_id,
            None,
            format!("{} agent working…", role.as_str()),
        );
        wait_for_settle(&state.bus, session.id, grace, timeout).await;
        teardown_session(state, &session).await;

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
    let mut derived = plan_from_spec(workdir, &spec_id).await?;
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
async fn await_repl_ready(state: &AppState, session: &agentum_core::Session) {
    let host = match state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await
    {
        Ok(Some(h)) => h,
        _ => return,
    };
    // We can only cheaply capture local panes; remote panes get a fixed delay.
    if !matches!(host.kind, HostKind::Local) {
        tokio::time::sleep(AGENT_BOOT_DELAY).await;
        return;
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
            return;
        }
    }
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
pub(crate) async fn inject_prompt(
    state: &AppState,
    session: &agentum_core::Session,
    prompt: &str,
) -> anyhow::Result<()> {
    await_repl_ready(state, session).await;
    let host = state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await?
        .ok_or_else(|| anyhow::anyhow!("session host missing"))?;
    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
    // Step 1: type/paste the prompt body, no Enter.
    crate::host_runtime::send_keys(&host, &target, prompt, false)
        .await
        .map_err(|e| anyhow::anyhow!("send_keys failed: {e}"))?;
    // Step 2: let the paste settle, then submit with a bare Enter.
    tokio::time::sleep(SUBMIT_DELAY).await;
    crate::host_runtime::send_keys(&host, &target, "", true)
        .await
        .map_err(|e| anyhow::anyhow!("submit Enter failed: {e}"))?;
    Ok(())
}

/// Gracefully stop an agent's pane + mark the session stopped. Best-effort.
async fn teardown_session(state: &AppState, session: &agentum_core::Session) {
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
        let _ = crate::host_runtime::kill_session(&host, &target).await;
        let _ = state
            .store
            .update_status_and_target(session.id, Status::Stopped, None)
            .await;
    }
}

/// Wait until the agent in `session_id` looks done with its turn — the first
/// `agent.awaiting_input` / `agent.finished` event after `grace` — or until
/// `timeout` elapses (then we run the gate anyway). A crash/stop also returns.
async fn wait_for_settle(
    bus: &broadcast::Sender<Event>,
    session_id: Uuid,
    grace: Duration,
    timeout: Duration,
) {
    let mut rx = bus.subscribe();
    let start = Instant::now();
    // Did the agent go idle *inside* the grace window? If so we don't discard
    // that signal (which would strand a fast feature waiting out the full
    // `timeout`) — we return as soon as grace elapses.
    let mut settled_early = false;
    loop {
        if settled_early && start.elapsed() >= grace {
            return;
        }
        let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
            return; // overall settle timeout — proceed to the gate
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
                    return;
                }
                continue;
            }
            Ok(Ok(ev)) => {
                if ev.session_id != Some(session_id) {
                    continue;
                }
                match ev.kind.as_str() {
                    "agent.awaiting_input" | "agent.finished" => {
                        if start.elapsed() >= grace {
                            return;
                        }
                        // The agent's *initial* idle, before grace — remember it
                        // and return once the grace window closes.
                        settled_early = true;
                    }
                    "session.crashed" | "session.stopped" => return,
                    _ => {}
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a `.harness/` with the given verify.sh body and two pending features.
    async fn setup(verify_body: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let harness_dir = dir.path().join(".harness");
        std::fs::create_dir_all(&harness_dir).unwrap();

        let features = FeatureList {
            features: vec![
                Feature {
                    id: "feat-1".into(),
                    name: "Feature One".into(),
                    description: "First feature".into(),
                    state: FeatureState::Pending,
                    attempts: 0,
                    last_error: None,
                    prompt: None,
                    tracker_provider: None,
                    tracker_url: None,
                },
                Feature {
                    id: "feat-2".into(),
                    name: "Feature Two".into(),
                    description: "Second feature".into(),
                    state: FeatureState::Pending,
                    attempts: 0,
                    last_error: None,
                    prompt: None,
                    tracker_provider: None,
                    tracker_url: None,
                },
            ],
            max_retries: 2,
            ..Default::default()
        };
        std::fs::write(
            harness_dir.join("feature_list.json"),
            serde_json::to_string_pretty(&features).unwrap(),
        )
        .unwrap();
        std::fs::write(
            harness_dir.join("AGENTS.md"),
            "# Agent Instructions\n\nBuild it.",
        )
        .unwrap();
        std::fs::write(harness_dir.join("verify.sh"), verify_body).unwrap();

        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[tokio::test]
    async fn loads_config_with_defaults() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let cfg = HarnessConfig::load(&wd).await.unwrap();
        assert_eq!(cfg.features.features.len(), 2);
        assert_eq!(cfg.features.agent_tool, "claude"); // default
        assert!(cfg.agent_instructions.contains("Agent Instructions"));
    }

    #[tokio::test]
    async fn missing_harness_dir_errors() {
        let dir = TempDir::new().unwrap();
        assert!(HarnessConfig::load(dir.path()).await.is_err());
    }

    #[tokio::test]
    async fn start_then_next_pending() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();
        assert_eq!(engine.status(id).await.unwrap().state, HarnessState::Idle);
        assert_eq!(
            engine.next_pending_feature(id).await.unwrap().unwrap().id,
            "feat-1"
        );
    }

    // Drives the bash `verify.sh`/`qa.sh` gate — the Harness Engine is Unix-shell-based.
    #[cfg(unix)]
    #[tokio::test]
    async fn green_gate_marks_done_and_writes_handoff() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let engine = HarnessEngine::new();
        let id = engine.start(wd.clone()).await.unwrap();

        let passed = engine.run_verify(id, "feat-1").await.unwrap();
        assert!(passed);

        let status = engine.status(id).await.unwrap();
        let f1 = status
            .features
            .features
            .iter()
            .find(|f| f.id == "feat-1")
            .unwrap();
        assert_eq!(f1.state, FeatureState::Done);
        assert!(wd.join(".harness/handoff.md").exists());
    }

    /// Write a `qa.sh` into an existing harness workdir created by `setup`.
    fn write_qa(wd: &Path, body: &str) {
        std::fs::write(resolve_harness_dir(wd).join("qa.sh"), body).unwrap();
    }

    // Drives the bash `verify.sh`/`qa.sh` gate — the Harness Engine is Unix-shell-based.
    #[cfg(unix)]
    #[tokio::test]
    async fn qa_gate_passes_and_marks_ready_to_test() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        write_qa(&wd, "#!/bin/bash\nexit 0\n");
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();

        let (ok, _out) = engine.run_qa_once(id, "feat-1").await.unwrap();
        assert!(ok, "qa.sh exit 0 → QA gate green");
        // run_qa_once flips the feature to ReadyToTest so the board mirrors it.
        let f = engine.status(id).await.unwrap().features.features[0].clone();
        assert_eq!(f.state, FeatureState::ReadyToTest);
    }

    // Drives the bash `verify.sh`/`qa.sh` gate — the Harness Engine is Unix-shell-based.
    #[cfg(unix)]
    #[tokio::test]
    async fn qa_gate_reports_failure() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        write_qa(&wd, "#!/bin/bash\necho 'pixel mismatch' >&2\nexit 1\n");
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();

        let (ok, out) = engine.run_qa_once(id, "feat-1").await.unwrap();
        assert!(!ok, "qa.sh non-zero → QA gate red");
        assert!(out.contains("pixel mismatch"));
    }

    #[tokio::test]
    async fn qa_gate_absent_is_a_pass() {
        // `setup` writes no qa.sh — a project with no browser surface isn't blocked.
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();
        let (ok, out) = engine.run_qa_once(id, "feat-1").await.unwrap();
        assert!(ok, "no qa.sh → QA gate skipped (pass)");
        assert!(out.contains("skipped"));
    }

    #[tokio::test]
    async fn ready_to_test_state_serializes_snake_case() {
        let json = serde_json::to_string(&FeatureState::ReadyToTest).unwrap();
        assert_eq!(json, "\"ready_to_test\"");
    }

    #[test]
    fn parse_qa_verdict_reads_pass_fail_and_summary() {
        let (p, s) = parse_qa_verdict(r#"{"passed": true, "summary": "login works"}"#).unwrap();
        assert!(p);
        assert_eq!(s, "login works");

        let (p, s) = parse_qa_verdict(r#"{"passed": false}"#).unwrap();
        assert!(!p);
        assert_eq!(s, "");

        // A non-verdict (or empty/garbled) is an error → the caller fails the gate.
        assert!(parse_qa_verdict("not json").is_err());
        assert!(
            parse_qa_verdict("{}").is_err(),
            "missing `passed` must error"
        );
    }

    #[test]
    fn qa_verdict_path_is_under_qa_dir_and_sanitized() {
        let p = qa_verdict_path(Path::new("/proj/.agentum-harness"), "AG/12");
        assert!(p.ends_with("qa/AG-12.json"), "got {}", p.display());
    }

    #[test]
    fn qa_mode_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&QaMode::Auto).unwrap(), "\"auto\"");
        assert_eq!(serde_json::to_string(&QaMode::Agent).unwrap(), "\"agent\"");
        assert_eq!(
            serde_json::to_string(&QaMode::Script).unwrap(),
            "\"script\""
        );
    }

    #[tokio::test]
    async fn resolve_qa_mode_honors_explicit_and_auto() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        // Auto with no qa.sh and (assumed) no browser-verify → Script (skip-pass).
        let mut cfg = HarnessConfig::load(&wd).await.unwrap();
        cfg.features.qa_mode = QaMode::Auto;
        assert!(matches!(
            resolve_qa_mode(&cfg),
            QaMode::Script | QaMode::Agent
        ));

        // Explicit overrides ignore detection.
        cfg.features.qa_mode = QaMode::Agent;
        assert_eq!(resolve_qa_mode(&cfg), QaMode::Agent);
        cfg.features.qa_mode = QaMode::Script;
        assert_eq!(resolve_qa_mode(&cfg), QaMode::Script);

        // Auto WITH a qa.sh present → Script (an explicit script wins over an agent).
        write_qa(&wd, "#!/bin/bash\nexit 0\n");
        let mut cfg2 = HarnessConfig::load(&wd).await.unwrap();
        cfg2.features.qa_mode = QaMode::Auto;
        assert_eq!(resolve_qa_mode(&cfg2), QaMode::Script);
    }

    // Drives the bash `verify.sh`/`qa.sh` gate — the Harness Engine is Unix-shell-based.
    #[cfg(unix)]
    #[tokio::test]
    async fn red_gate_retries_then_blocks() {
        // Always fails → after max_retries (2) the feature is Blocked.
        let (_d, wd) = setup("#!/bin/bash\necho boom >&2\nexit 1\n").await;
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();

        // First failure: attempts=1 < 2 → not blocked, left Coding.
        let (ok1, out1) = engine.run_verify_once(id, "feat-1").await.unwrap();
        assert!(!ok1);
        assert!(out1.contains("boom"));
        let blocked1 = engine
            .record_feature_failure(id, "feat-1", &out1)
            .await
            .unwrap();
        assert!(!blocked1);
        let f = engine.status(id).await.unwrap().features.features[0].clone();
        assert_eq!(f.state, FeatureState::Coding);
        assert_eq!(f.attempts, 1);

        // Second failure: attempts=2 >= 2 → Blocked.
        let (_ok2, out2) = engine.run_verify_once(id, "feat-1").await.unwrap();
        let blocked2 = engine
            .record_feature_failure(id, "feat-1", &out2)
            .await
            .unwrap();
        assert!(blocked2);
        let f = engine.status(id).await.unwrap().features.features[0].clone();
        assert_eq!(f.state, FeatureState::Blocked);
        assert!(f.last_error.is_some());
    }

    #[tokio::test]
    async fn events_are_broadcast() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let engine = HarnessEngine::new();
        let mut rx = engine.subscribe();
        let id = engine.start(wd).await.unwrap();
        // The StateChanged(Idle) emitted by start() should arrive.
        let ev = rx.recv().await.unwrap();
        matches!(
            ev,
            HarnessEvent::StateChanged {
                state: HarnessState::Idle,
                ..
            }
        );
        let _ = id;
    }

    #[test]
    fn tail_snaps_to_char_boundary() {
        let s = "a".repeat(10);
        assert_eq!(tail(&s, 100), s);
        assert!(tail(&s, 3).ends_with("aaa"));
    }

    fn feat(id: &str) -> Feature {
        Feature {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: None,
            tracker_url: None,
        }
    }

    /// Write a `feature_list.json` with the given features into a fresh
    /// `.harness/` (no scripts needed — these exercise load/validation only).
    fn write_features(features: Vec<Feature>) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let harness_dir = dir.path().join(".harness");
        std::fs::create_dir_all(&harness_dir).unwrap();
        let list = FeatureList {
            features,
            ..Default::default()
        };
        std::fs::write(
            harness_dir.join("feature_list.json"),
            serde_json::to_string_pretty(&list).unwrap(),
        )
        .unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[tokio::test]
    async fn empty_feature_list_rejected() {
        let (_d, wd) = write_features(vec![]);
        let err = HarnessConfig::load(&wd).await.unwrap_err().to_string();
        assert!(err.contains("no features"), "got: {err}");
    }

    #[tokio::test]
    async fn duplicate_feature_ids_rejected() {
        let (_d, wd) = write_features(vec![feat("dup"), feat("dup")]);
        let err = HarnessConfig::load(&wd).await.unwrap_err().to_string();
        assert!(err.contains("duplicate feature id"), "got: {err}");
    }

    #[tokio::test]
    async fn agent_yolo_defaults_on() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let cfg = HarnessConfig::load(&wd).await.unwrap();
        assert!(cfg.features.agent_yolo, "YOLO must default on for autonomy");
    }

    #[tokio::test]
    async fn claim_release_driver_round_trips() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();
        assert!(engine.claim_driver(id).await.unwrap(), "first claim wins");
        assert!(
            !engine.claim_driver(id).await.unwrap(),
            "second claim is rejected while driving"
        );
        engine.release_driver(id).await;
        assert!(
            engine.claim_driver(id).await.unwrap(),
            "re-claimable after release"
        );
    }

    #[tokio::test]
    async fn reset_blocked_features_retries_the_halted_feature() {
        // verify.sh always red → drive feat-1 into Blocked (max_retries = 2).
        let (_d, wd) = setup("#!/bin/bash\necho boom >&2\nexit 1\n").await;
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();
        for _ in 0..2 {
            let (_ok, out) = engine.run_verify_once(id, "feat-1").await.unwrap();
            engine
                .record_feature_failure(id, "feat-1", &out)
                .await
                .unwrap();
        }
        assert_eq!(
            engine.status(id).await.unwrap().features.features[0].state,
            FeatureState::Blocked
        );
        // next_pending would otherwise skip the blocked feature → feat-2.
        assert_eq!(
            engine.next_pending_feature(id).await.unwrap().unwrap().id,
            "feat-2"
        );

        let n = engine.reset_blocked_features(id).await.unwrap();
        assert_eq!(n, 1);
        let f = engine.status(id).await.unwrap().features.features[0].clone();
        assert_eq!(f.state, FeatureState::Pending);
        assert_eq!(f.attempts, 0);
        assert!(f.last_error.is_none());
        // After reset the halted feature is the next one to retry.
        assert_eq!(
            engine.next_pending_feature(id).await.unwrap().unwrap().id,
            "feat-1"
        );
    }

    #[tokio::test]
    async fn clear_current_drops_pointers() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let engine = HarnessEngine::new();
        let id = engine.start(wd).await.unwrap();
        engine
            .set_session(id, Uuid::new_v4(), "feat-1")
            .await
            .unwrap();
        assert!(engine.status(id).await.unwrap().current_feature.is_some());
        engine.clear_current(id).await;
        let s = engine.status(id).await.unwrap();
        assert!(s.current_feature.is_none());
        assert!(s.current_session.is_none());
    }

    #[tokio::test]
    async fn settle_returns_after_grace_on_early_idle() {
        // The agent goes idle well inside the grace window. The wait must end
        // shortly after grace — NOT wait out the (here very long) settle timeout.
        let (tx, _keepalive) = broadcast::channel::<Event>(16);
        let sid = Uuid::new_v4();
        let grace = Duration::from_millis(150);
        let long_timeout = Duration::from_secs(60);

        let tx2 = tx.clone();
        let emitter = tokio::spawn(async move {
            // Let wait_for_settle subscribe first, then emit an early finish.
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = tx2.send(Event::new("agent.finished").with_session(sid, "s"));
        });

        let begin = Instant::now();
        wait_for_settle(&tx, sid, grace, long_timeout).await;
        let elapsed = begin.elapsed();
        emitter.await.unwrap();

        assert!(
            elapsed >= grace,
            "must honor the grace minimum, got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "must not wait out the full timeout on an early settle, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn settle_ignores_events_for_other_sessions() {
        let (tx, _keepalive) = broadcast::channel::<Event>(16);
        let sid = Uuid::new_v4();
        let other = Uuid::new_v4();
        let grace = Duration::from_millis(50);
        let timeout = Duration::from_millis(400);

        let tx2 = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            // An unrelated session finishing must NOT release our wait.
            let _ = tx2.send(Event::new("agent.finished").with_session(other, "x"));
        });

        let begin = Instant::now();
        wait_for_settle(&tx, sid, grace, timeout).await;
        // No event for `sid` ever arrives → we fall through at the settle timeout.
        assert!(begin.elapsed() >= timeout, "should wait out the timeout");
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;
    use tempfile::TempDir;

    fn bf(id: &str, name: &str, desc: &str, provider: Option<&str>) -> BacklogFeature {
        BacklogFeature {
            id: id.into(),
            name: name.into(),
            description: desc.into(),
            provider: provider.map(|s| s.to_string()),
            url: None,
        }
    }

    #[test]
    fn resolve_prefers_canonical_then_legacy() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        // neither exists → canonical path returned (callers scaffold).
        assert_eq!(resolve_harness_dir(wd), wd.join(HARNESS_DIR));
        // only legacy exists → legacy.
        std::fs::create_dir_all(wd.join(LEGACY_HARNESS_DIR)).unwrap();
        assert_eq!(resolve_harness_dir(wd), wd.join(LEGACY_HARNESS_DIR));
        // canonical present → canonical wins over legacy.
        std::fs::create_dir_all(wd.join(HARNESS_DIR)).unwrap();
        assert_eq!(resolve_harness_dir(wd), wd.join(HARNESS_DIR));
    }

    #[tokio::test]
    async fn scaffold_creates_loadable_surface() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let out = scaffold_harness(wd).await.unwrap();
        assert!(wd.join(".agentum-harness/feature_list.json").exists());
        assert!(wd.join(".agentum-harness/AGENTS.md").exists());
        assert!(out.written.iter().any(|w| w.contains("feature_list.json")));
        // the scaffolded surface loads through the normal engine path.
        let cfg = HarnessConfig::load(wd).await.unwrap();
        assert_eq!(cfg.harness_dir, wd.join(HARNESS_DIR));
        assert!(!cfg.features.features.is_empty());
        // idempotent: a second scaffold writes nothing new.
        let again = scaffold_harness(wd).await.unwrap();
        assert!(again.written.is_empty());
    }

    #[tokio::test]
    async fn migrate_maps_legacy_and_specs() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        // a legacy .harness with a (minimal) feature list
        std::fs::create_dir_all(wd.join(".harness")).unwrap();
        std::fs::write(wd.join(".harness/feature_list.json"), "{\"features\":[]}").unwrap();
        // an SDD spec deliverable
        std::fs::create_dir_all(wd.join("ai/specs/001-demo")).unwrap();
        std::fs::write(wd.join("ai/specs/001-demo/spec.md"), "# Spec").unwrap();

        let out = migrate_harness(wd, false).await.unwrap();
        assert!(wd.join(".agentum-harness/feature_list.json").exists());
        assert!(wd.join(".agentum-harness/specs/001-demo/spec.md").exists());
        assert!(
            wd.join(".harness").exists(),
            "legacy kept when remove_legacy=false"
        );
        assert!(!out.written.is_empty());
    }

    #[tokio::test]
    async fn load_still_reads_legacy_harness() {
        // Back-compat: a project with only `.harness/` (no `.agentum-harness/`)
        // must still load — this is what keeps the demo + in-flight worktrees green.
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        std::fs::create_dir_all(wd.join(".harness")).unwrap();
        std::fs::write(
            wd.join(".harness/feature_list.json"),
            "{\"features\":[{\"id\":\"F1\",\"name\":\"x\"}]}",
        )
        .unwrap();
        let cfg = HarnessConfig::load(wd).await.unwrap();
        assert_eq!(cfg.harness_dir, wd.join(LEGACY_HARNESS_DIR));
    }

    // --- 010b: per-worktree rebuildable board ---

    #[tokio::test]
    async fn board_empty_when_no_surface() {
        let dir = TempDir::new().unwrap();
        let board = scan_board(dir.path()).await;
        assert!(board.harness_dir.is_none());
        assert!(board.specs.is_empty());
        assert!(board.features.is_empty());
    }

    #[tokio::test]
    async fn board_reflects_scaffold_and_migrate() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        // scaffold seeds one feature → board sees it (read purely from disk).
        scaffold_harness(wd).await.unwrap();
        let board = scan_board(wd).await;
        assert!(board.harness_dir.is_some());
        assert_eq!(
            board.features.len(),
            1,
            "seeded feature visible on the board"
        );
        // add an SDD spec + migrate it → board lists the spec deliverable.
        std::fs::create_dir_all(wd.join("ai/specs/001-demo")).unwrap();
        std::fs::write(wd.join("ai/specs/001-demo/spec.md"), "# Spec").unwrap();
        migrate_harness(wd, false).await.unwrap();
        let board = scan_board(wd).await;
        let demo = board.specs.iter().find(|s| s.id == "001-demo");
        assert!(demo.is_some(), "migrated spec appears on the board");
        assert!(demo.unwrap().has_spec, "spec.md detected on disk");
    }

    // --- 010c: spec→backlog pipeline ---

    #[test]
    fn derive_backlog_maps_checkboxes() {
        let spec = "# Spec\n\n## Acceptance Criteria\n\n\
            - [ ] First criterion\n\
            - [x] Already done criterion\n\
            - [ ] Third criterion\n\n\
            Some prose, not a checkbox.\n";
        let list = derive_backlog_from_spec(spec);
        assert_eq!(list.features.len(), 3, "one feature per checkbox");
        assert_eq!(list.features[0].id, "F1");
        assert_eq!(list.features[0].name, "First criterion");
        assert_eq!(list.features[0].state, FeatureState::Pending);
        assert_eq!(list.features[1].state, FeatureState::Done, "[x] → Done");
        assert_eq!(list.features[2].id, "F3");
    }

    #[tokio::test]
    async fn plan_from_spec_writes_loadable_backlog() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let spec_dir = wd.join(".agentum-harness/specs/s1");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(
            spec_dir.join("spec.md"),
            "## Acceptance Criteria\n- [ ] A\n- [ ] B\n",
        )
        .unwrap();

        let list = plan_from_spec(wd, "s1").await.unwrap();
        assert_eq!(list.features.len(), 2);
        // written feature_list.json is loadable by the engine + visible on the board.
        let cfg = HarnessConfig::load(wd).await.unwrap();
        assert_eq!(cfg.features.features.len(), 2);
        let board = scan_board(wd).await;
        assert_eq!(board.features.len(), 2);
    }

    #[tokio::test]
    async fn plan_rejects_spec_without_criteria() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let spec_dir = wd.join(".agentum-harness/specs/s1");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(spec_dir.join("spec.md"), "# Spec\n\nNo checkboxes here.\n").unwrap();
        assert!(
            plan_from_spec(wd, "s1").await.is_err(),
            "no criteria → explicit error, not a silent empty backlog"
        );
    }

    #[tokio::test]
    async fn write_backlog_from_features_writes_loadable_idle_backlog() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let feats = vec![
            bf("AG-1", "Add login", "user can log in", Some("board")),
            bf("AG-2", "Add logout", "", Some("board")),
        ];
        let list = write_backlog_from_features(wd, &feats).await.unwrap();

        // Every feature is Pending — the harness is Idle, not running.
        assert_eq!(list.features.len(), 2);
        assert!(
            list.features
                .iter()
                .all(|f| f.state == FeatureState::Pending)
        );
        // Tracker key is reused verbatim as the harness feature id.
        assert_eq!(list.features[0].id, "AG-1");
        assert_eq!(list.features[0].description, "user can log in");
        // Tracker provenance round-trips so lifecycle transitions know the sink.
        assert_eq!(list.features[0].tracker_provider.as_deref(), Some("board"));

        // The written file is loadable by the engine and visible on the board.
        let cfg = HarnessConfig::load(wd).await.unwrap();
        assert_eq!(cfg.features.features.len(), 2);
        let board = scan_board(wd).await;
        assert_eq!(board.features.len(), 2);
    }

    #[tokio::test]
    async fn write_backlog_from_features_rejects_empty() {
        let dir = TempDir::new().unwrap();
        assert!(
            write_backlog_from_features(dir.path(), &[]).await.is_err(),
            "empty input → explicit error, never a silent empty backlog"
        );
    }

    #[tokio::test]
    async fn write_backlog_from_features_rejects_duplicate_ids() {
        let dir = TempDir::new().unwrap();
        let feats = vec![bf("AG-1", "a", "", None), bf("AG-1", "b", "", None)];
        assert!(
            write_backlog_from_features(dir.path(), &feats)
                .await
                .is_err(),
            "duplicate ids would make state writes target the wrong feature"
        );
    }

    // --- 010c slice 2: HITL-at-QA gate ---

    // Drives the bash `verify.sh`/`qa.sh` gate — the Harness Engine is Unix-shell-based.
    #[cfg(unix)]
    #[tokio::test]
    async fn hitl_at_qa_parks_then_confirm_finalizes() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let hd = wd.join(".agentum-harness");
        std::fs::create_dir_all(&hd).unwrap();
        std::fs::write(
            hd.join("feature_list.json"),
            r#"{"features":[{"id":"F1","name":"x"}],"hitl_at_qa":true}"#,
        )
        .unwrap();
        std::fs::write(hd.join("verify.sh"), "#!/usr/bin/env bash\nexit 0\n").unwrap();

        let engine = HarnessEngine::new();
        let id = engine.start(wd.to_path_buf()).await.unwrap();

        // Verify passes, but HITL-at-QA PARKS it at AwaitingConfirm — not Done.
        assert!(engine.run_verify(id, "F1").await.unwrap(), "verify passed");
        assert_eq!(
            scan_board(wd).await.features[0].state,
            FeatureState::AwaitingConfirm,
            "parked for human confirmation, persisted to disk"
        );

        // Confirming finalizes it to Done.
        engine.confirm_feature(id, "F1").await.unwrap();
        assert_eq!(scan_board(wd).await.features[0].state, FeatureState::Done);

        // A stray confirm on a non-awaiting (now Done) feature errors.
        assert!(
            engine.confirm_feature(id, "F1").await.is_err(),
            "confirm guard: only AwaitingConfirm can be confirmed"
        );
    }

    // Drives the bash `verify.sh`/`qa.sh` gate — the Harness Engine is Unix-shell-based.
    #[cfg(unix)]
    #[tokio::test]
    async fn no_hitl_marks_done_directly() {
        // Default (hitl_at_qa absent/false): green verify → Done immediately.
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let hd = wd.join(".agentum-harness");
        std::fs::create_dir_all(&hd).unwrap();
        std::fs::write(
            hd.join("feature_list.json"),
            r#"{"features":[{"id":"F1","name":"x"}]}"#,
        )
        .unwrap();
        std::fs::write(hd.join("verify.sh"), "#!/usr/bin/env bash\nexit 0\n").unwrap();
        let engine = HarnessEngine::new();
        let id = engine.start(wd.to_path_buf()).await.unwrap();
        assert!(engine.run_verify(id, "F1").await.unwrap());
        assert_eq!(scan_board(wd).await.features[0].state, FeatureState::Done);
    }

    // --- 010d: Bootstrap-Contract readiness check ---

    #[tokio::test]
    async fn bootstrap_ready_after_scaffold_and_gaps() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        // empty surface → not ready, no error.
        assert!(!check_bootstrap(wd).await.ready);

        // scaffold writes all four contract items → ready.
        scaffold_harness(wd).await.unwrap();
        let r = check_bootstrap(wd).await;
        assert!(
            r.ready && r.agents_md && r.init_sh && r.verify_sh && r.backlog,
            "scaffolded surface satisfies the Bootstrap Contract"
        );

        // remove verify.sh → not ready; names the gap; others still true.
        std::fs::remove_file(wd.join(".agentum-harness/verify.sh")).unwrap();
        let r = check_bootstrap(wd).await;
        assert!(!r.ready && !r.verify_sh && r.agents_md && r.init_sh);
    }

    #[tokio::test]
    async fn bootstrap_backlog_false_when_no_features() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let hd = wd.join(".agentum-harness");
        std::fs::create_dir_all(&hd).unwrap();
        std::fs::write(hd.join("feature_list.json"), r#"{"features":[]}"#).unwrap();
        let r = check_bootstrap(wd).await;
        assert!(
            !r.backlog && !r.ready,
            "empty backlog → not bootstrap-ready"
        );
    }

    // --- 010e: append-only decision log ---

    #[tokio::test]
    async fn decision_log_is_append_only() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        assert_eq!(read_decisions(wd).await, "", "no log yet");
        append_decision(wd, "chose per-repo durable specs over agentum-only")
            .await
            .unwrap();
        append_decision(wd, "rejected agentum-only storage (data-loss risk)")
            .await
            .unwrap();
        let log = read_decisions(wd).await;
        let first = log.find("chose per-repo").expect("first entry present");
        let second = log.find("rejected agentum-only").expect("second present");
        assert!(first < second, "append-only, kept in order");
        assert!(wd.join(".agentum-harness/decisions.md").exists());
    }

    // ---- spec 013: autonomous SDD role phases -----------------------------

    #[test]
    fn decide_gate_advances_on_pass() {
        assert_eq!(decide_gate(true, 1, 3, false), GateDecision::Advance);
        // A pass advances regardless of attempt / hitl.
        assert_eq!(decide_gate(true, 9, 0, true), GateDecision::Advance);
    }

    #[test]
    fn decide_gate_retries_while_budget_remains() {
        assert_eq!(decide_gate(false, 1, 3, false), GateDecision::Retry);
        assert_eq!(decide_gate(false, 3, 3, false), GateDecision::Retry);
    }

    #[test]
    fn decide_gate_blocks_when_exhausted_and_autonomous() {
        assert_eq!(decide_gate(false, 4, 3, false), GateDecision::Block);
        assert_eq!(decide_gate(false, 100, 3, false), GateDecision::Block);
    }

    #[test]
    fn decide_gate_awaits_human_only_when_hitl_on_block() {
        assert_eq!(decide_gate(false, 4, 3, true), GateDecision::AwaitConfirm);
    }

    /// The autonomy contract: with `hitl_on_block` off (the default) the engine
    /// NEVER asks a human — across the entire failure space it blocks, never
    /// awaits. This is the core "don't prompt me to continue" guarantee (013).
    #[test]
    fn decide_gate_default_run_never_prompts_a_human() {
        for attempt in 1..=10u32 {
            for max in 0..=5u32 {
                assert_ne!(
                    decide_gate(false, attempt, max, false),
                    GateDecision::AwaitConfirm,
                    "autonomous run must never await a human (attempt={attempt}, max={max})"
                );
            }
        }
    }

    #[test]
    fn parse_role_verdict_reads_pass_fail_and_summary() {
        let (p, s) = parse_role_verdict(r#"{"passed":true,"summary":"looks good"}"#).unwrap();
        assert!(p);
        assert_eq!(s, "looks good");
        let (p, s) = parse_role_verdict(r#"{"passed":false,"summary":"missing AC"}"#).unwrap();
        assert!(!p);
        assert_eq!(s, "missing AC");
    }

    #[test]
    fn parse_role_verdict_summary_is_optional() {
        let (p, s) = parse_role_verdict(r#"{"passed":true}"#).unwrap();
        assert!(p);
        assert_eq!(s, "");
    }

    #[test]
    fn parse_role_verdict_rejects_garbage() {
        // An inconclusive/garbled verdict must error so the caller fails the gate
        // — it must never silently read as a pass.
        assert!(parse_role_verdict("not json").is_err());
        assert!(parse_role_verdict("").is_err());
        assert!(parse_role_verdict("{}").is_err());
    }

    #[test]
    fn build_role_prompt_includes_brief_spec_and_verdict_contract() {
        let p = build_role_prompt(
            RoleKind::Pm,
            "AGENTS instructions",
            "013-x",
            "# Spec body",
            "roles/authoring.json",
        );
        assert!(p.contains("Product Manager"), "embeds the PM brief");
        assert!(p.contains("# Spec body"), "embeds the spec");
        assert!(p.contains("013-x"), "names the spec id");
        assert!(p.contains("roles/authoring.json"), "names the verdict path");
        assert!(p.contains("\"passed\""), "states the JSON verdict contract");
    }

    #[test]
    fn role_briefs_are_embedded_and_demand_a_verdict() {
        for role in [RoleKind::Pm, RoleKind::Architect, RoleKind::Reviewer] {
            let b = role.brief();
            assert!(!b.trim().is_empty(), "{} brief is embedded", role.as_str());
            assert!(
                b.contains("verdict") && b.contains("passed"),
                "{} brief states the verdict contract",
                role.as_str()
            );
        }
    }

    #[test]
    fn spec_phase_advances_through_the_lifecycle() {
        use SpecPhase::*;
        assert_eq!(Authoring.advance(), Architecture);
        assert_eq!(Architecture.advance(), Decompose);
        assert_eq!(Decompose.advance(), Executing);
        assert_eq!(Executing.advance(), Review);
        assert_eq!(Review.advance(), Done);
        assert_eq!(Done.advance(), Done, "terminal phase stays put");
    }

    #[test]
    fn spec_phase_role_mapping_is_correct() {
        assert_eq!(SpecPhase::Authoring.role(), Some(RoleKind::Pm));
        assert_eq!(SpecPhase::Architecture.role(), Some(RoleKind::Architect));
        assert_eq!(SpecPhase::Review.role(), Some(RoleKind::Reviewer));
        assert_eq!(SpecPhase::Decompose.role(), None);
        assert_eq!(SpecPhase::Executing.role(), None);
        assert_eq!(SpecPhase::Done.role(), None);
    }

    #[test]
    fn spec_phase_slug_round_trips() {
        for p in [
            SpecPhase::Authoring,
            SpecPhase::Architecture,
            SpecPhase::Decompose,
            SpecPhase::Executing,
            SpecPhase::Review,
            SpecPhase::Done,
            SpecPhase::Blocked,
            SpecPhase::AwaitingConfirm,
        ] {
            assert_eq!(SpecPhase::from_slug(p.slug()), Some(p));
        }
        assert_eq!(SpecPhase::from_slug("nonsense"), None);
    }

    #[test]
    fn rebuild_phase_takes_the_last_marker() {
        let log = "phase: entered authoring (from executing)\n\
                   note: pm refined the spec\n\
                   phase: entered architecture (from authoring)\n";
        assert_eq!(
            rebuild_phase_from_decisions(log),
            Some(SpecPhase::Architecture)
        );
    }

    #[test]
    fn rebuild_phase_is_none_when_absent() {
        assert_eq!(rebuild_phase_from_decisions("no markers here"), None);
        assert_eq!(rebuild_phase_from_decisions(""), None);
    }

    #[test]
    fn role_verdict_path_is_under_roles_dir() {
        let p = role_verdict_path(Path::new("/x/.agentum-harness"), SpecPhase::Authoring);
        assert!(p.ends_with("roles/authoring.json"), "got {p:?}");
    }

    #[test]
    fn default_feature_list_keeps_roles_off_and_autonomous() {
        let f = FeatureList::default();
        assert!(
            !f.roles,
            "role phases off by default — feature-only runs unchanged"
        );
        assert!(!f.hitl_on_block, "fully autonomous by default");
        assert!(f.spec_id.is_none());
    }

    #[test]
    fn copy_knobs_preserves_config_but_not_features() {
        let mut src = FeatureList {
            spec_id: Some("013-x".into()),
            roles: true,
            hitl_on_block: true,
            agent_tool: "codex".into(),
            max_retries: 9,
            ..FeatureList::default()
        };
        // A knob source carries no meaningful features.
        src.features.clear();

        let mut derived = FeatureList {
            features: vec![Feature {
                id: "F1".into(),
                name: "f".into(),
                description: String::new(),
                state: FeatureState::Pending,
                attempts: 0,
                last_error: None,
                prompt: None,
                tracker_provider: None,
                tracker_url: None,
            }],
            ..FeatureList::default()
        };
        derived.copy_knobs_from(&src);

        assert_eq!(derived.spec_id.as_deref(), Some("013-x"));
        assert!(derived.roles);
        assert!(derived.hitl_on_block);
        assert_eq!(derived.agent_tool, "codex");
        assert_eq!(derived.max_retries, 9);
        assert_eq!(
            derived.features.len(),
            1,
            "the derived backlog's features are preserved, not overwritten"
        );
    }
}
