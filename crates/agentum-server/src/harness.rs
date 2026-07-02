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
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

// Harness data types + on-disk `.agentum-harness/` operations live in `types`;
// the engine, drive loop, and gate helpers below still reference them directly
// via this glob re-export (which also preserves the `harness::Foo` public API).
mod types;
pub use types::*;

// Prompt builders + verdict parsers + small utilities; `pub(crate)` items the
// drive/gate code calls. Internal (not re-exported in the public surface).
mod helpers;
use helpers::*;

// The drive loop + orchestration free functions. As a child module, `drive`
// calls `HarnessEngine`'s private methods directly (no widening needed). Only
// the two entry points are re-exported: `drive` (the run task, called by
// routes/harness.rs) publicly, and `inject_prompt` (used by routes/board_goals.rs)
// at crate visibility.
mod drive;
pub use drive::drive;
pub(crate) use drive::{inject_prompt, teardown_session, wait_for_settle};

/// Manages every concurrent harness run + the event bus they publish on.
pub struct HarnessEngine {
    runs: RwLock<HashMap<Uuid, Arc<RwLock<HarnessRun>>>>,
    event_tx: broadcast::Sender<HarnessEvent>,
    /// Serializes `POST /api/harness/start-work` end-to-end (spec 005 F1, C5):
    /// the already-running check, the `feature_list.json` rewrite, and the
    /// fresh register+claim must be atomic per workdir or two retries can
    /// register two drivers on one worktree (the per-run `claim_driver` can't
    /// see across registrations).
    pub(crate) start_work_lock: tokio::sync::Mutex<()>,
}

impl HarnessEngine {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(512);
        Self {
            runs: RwLock::new(HashMap::new()),
            event_tx,
            start_work_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Subscribe to the harness event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<HarnessEvent> {
        self.event_tx.subscribe()
    }

    /// The registered run (if any) whose workdir == `workdir`. First match
    /// wins; start-work keeps the map at ≤1 run per workdir going forward
    /// (spec 005 F1 — the friendly already-running resolution).
    pub async fn find_by_workdir(&self, workdir: &Path) -> Option<Uuid> {
        let runs = self.runs.read().await;
        for (id, run) in runs.iter() {
            if run.read().await.workdir == workdir {
                return Some(*id);
            }
        }
        None
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

#[cfg(test)]
mod tests {
    use super::*;
    // Types/fns these tests use that the (slimmed) non-test imports no longer
    // pull in, plus the two drive-internal fns under test (now in `drive`).
    use super::drive::{resolve_qa_mode, wait_for_settle};
    use agentum_core::Event;
    use std::path::Path;
    use std::time::Duration;
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

    /// Spec 005 F2 regression pin (AC 6): a backlog WITHOUT a `spec_id` must
    /// produce today's prompt byte-for-byte. The expected string is written out
    /// in full (no format!) so any drift in the load-bearing prompt is loud.
    #[test]
    fn feature_prompt_without_spec_is_byte_identical() {
        let feature = Feature {
            id: "F1".into(),
            name: "Add login".into(),
            description: "User can log in with email.".into(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: None,
            tracker_url: None,
        };
        let expected = "You are an agent running inside the Agentum Harness Engine.\n\
            \n\
            === HARNESS INSTRUCTIONS (AGENTS.md) ===\n\
            Build it well.\n\
            \n\
            === YOUR CURRENT TASK — EXACTLY ONE FEATURE ===\n\
            Feature: Add login\n\
            ID: F1\n\
            User can log in with email.\n\
            \n\
            Work ONLY on this feature. When you believe it is complete, stop and wait. \
            The harness will then run the verification gate (verify.sh). If verification \
            fails you will be given the error output and must fix it.";
        assert_eq!(
            build_feature_prompt("Build it well.", &feature, None),
            expected
        );
    }

    /// Spec 005 F2 (AC 6): a spec-planned backlog's prompt names the spec's
    /// relative path and tells the agent to read it first — while the gate
    /// contract text survives untouched.
    #[test]
    fn feature_prompt_with_spec_names_the_path_and_says_read_first() {
        let feature = Feature {
            id: "F1".into(),
            name: "Add login".into(),
            description: "User can log in with email.".into(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: None,
            tracker_url: None,
        };
        let rel = ".agentum-harness/specs/42-add-widget/spec.md";
        let p = build_feature_prompt("Build it well.", &feature, Some(rel));
        assert!(p.contains("=== THE SPEC ==="));
        assert!(p.contains(rel), "prompt must name the spec's relative path");
        assert!(p.contains("BEFORE coding"), "must say read-first");
        assert!(
            p.contains("Work ONLY on this feature"),
            "gate contract text must survive"
        );
        assert!(
            p.contains("verification gate (verify.sh)"),
            "gate contract text must survive"
        );
    }

    /// Spec 005 F2 second byte-identical pin: an explicit per-feature `prompt`
    /// override wins even when a spec path is supplied (the helpers.rs
    /// short-circuit stays FIRST and unconditional).
    #[test]
    fn feature_prompt_explicit_override_wins_even_with_spec() {
        let feature = Feature {
            id: "F1".into(),
            name: "Add login".into(),
            description: "ignored".into(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: Some("Do exactly this.".into()),
            tracker_provider: None,
            tracker_url: None,
        };
        assert_eq!(
            build_feature_prompt(
                "Build it well.",
                &feature,
                Some(".agentum-harness/specs/s1/spec.md")
            ),
            "Do exactly this."
        );
    }

    /// Spec 005 F3 (AC 7): the QA prompt steers the agent at the `agentum_browser`
    /// MCP tool (open + split = the visible in-app browser) and no longer
    /// INSTRUCTS using the browser-verification-loop skill (it may still name it
    /// in the "Do NOT use" steer) — while the verdict-file contract stays
    /// character-for-character (the contract-identical pin).
    #[test]
    fn qa_prompt_steers_agentum_browser() {
        let feature = Feature {
            id: "F1".into(),
            name: "Add login".into(),
            description: "User can log in.".into(),
            state: FeatureState::ReadyToTest,
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: None,
            tracker_url: None,
        };
        let rel = ".agentum-harness/qa/F1.json";
        let p = build_qa_prompt("Build it well.", &feature, rel);
        assert!(p.contains("`agentum_browser`"), "names the tool");
        assert!(p.contains("op `open`"), "starts with open");
        assert!(p.contains("split"), "mentions side-by-side placement");
        assert!(
            !p.contains("Use the `browser-verification-loop`"),
            "must no longer instruct the skill"
        );
        assert!(
            p.contains("Do NOT use the browser-verification-loop"),
            "warns off the old skill path"
        );
        // The verdict contract, byte-identical to pre-005:
        assert!(p.contains(rel), "names the verdict rel path");
        assert!(p.contains(
            "as exactly this JSON:\n{\"passed\": true|false, \"summary\": \"one line on what you verified or why it failed\"}"
        ));
        assert!(p.contains(
            "Set passed=false if ANY check fails or you cannot verify. Do not stop until the file is written."
        ));
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
        // Now that capability is an explicit parameter (spec 005 F3) the old
        // `Script | Agent` env-leakage tolerance tightens to exact asserts.
        // Auto with no qa.sh and agent-QA not capable → Script (skip-pass).
        let mut cfg = HarnessConfig::load(&wd).await.unwrap();
        cfg.features.qa_mode = QaMode::Auto;
        assert_eq!(resolve_qa_mode(&cfg, false), QaMode::Script);

        // Explicit overrides ignore detection.
        cfg.features.qa_mode = QaMode::Agent;
        assert_eq!(resolve_qa_mode(&cfg, false), QaMode::Agent);
        cfg.features.qa_mode = QaMode::Script;
        assert_eq!(resolve_qa_mode(&cfg, false), QaMode::Script);

        // Auto WITH a qa.sh present → Script (an explicit script wins over an agent).
        write_qa(&wd, "#!/bin/bash\nexit 0\n");
        let mut cfg2 = HarnessConfig::load(&wd).await.unwrap();
        cfg2.features.qa_mode = QaMode::Auto;
        assert_eq!(resolve_qa_mode(&cfg2, false), QaMode::Script);
    }

    /// Spec 005 F3 (AC 8): the full mode × qa.sh-present × capable decision
    /// table — pure, no env mutation. The `capable = false` column IS the D3
    /// byte-identical pin: before 005, `Auto` + no qa.sh + no
    /// AGENTUM_BROWSER_VERIFY resolved to `Script` (skip-pass); with the knob
    /// OFF (its default) that behavior must be reproduced exactly, so non-web
    /// projects and headless/CI are unchanged.
    #[tokio::test]
    async fn resolve_qa_mode_matrix() {
        // Two configs: qa.sh absent / present.
        let (_d1, wd_absent) = setup("#!/bin/bash\nexit 0\n").await;
        let (_d2, wd_present) = setup("#!/bin/bash\nexit 0\n").await;
        write_qa(&wd_present, "#!/bin/bash\nexit 0\n");
        let cfg_absent = HarnessConfig::load(&wd_absent).await.unwrap();
        let cfg_present = HarnessConfig::load(&wd_present).await.unwrap();
        assert!(cfg_absent.qa_script.is_none());
        assert!(cfg_present.qa_script.is_some());

        for (mode, qa_sh_present, capable, want) in [
            // Explicit modes ignore BOTH dimensions.
            (QaMode::Script, false, false, QaMode::Script),
            (QaMode::Script, false, true, QaMode::Script),
            (QaMode::Script, true, false, QaMode::Script),
            (QaMode::Script, true, true, QaMode::Script),
            (QaMode::Agent, false, false, QaMode::Agent),
            (QaMode::Agent, false, true, QaMode::Agent),
            (QaMode::Agent, true, false, QaMode::Agent),
            (QaMode::Agent, true, true, QaMode::Agent),
            // Auto + qa.sh → Script always (an explicit script wins).
            (QaMode::Auto, true, false, QaMode::Script),
            (QaMode::Auto, true, true, QaMode::Script),
            // Auto + no qa.sh → capable decides; not-capable = skip-pass Script.
            (QaMode::Auto, false, true, QaMode::Agent),
            (QaMode::Auto, false, false, QaMode::Script),
        ] {
            let mut cfg = if qa_sh_present {
                cfg_present.clone()
            } else {
                cfg_absent.clone()
            };
            cfg.features.qa_mode = mode;
            assert_eq!(
                resolve_qa_mode(&cfg, capable),
                want,
                "cell: mode={mode:?} qa.sh={qa_sh_present} capable={capable}"
            );
        }
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

    /// Spec 005 F1: start-work resolves an existing run for the target worktree
    /// by workdir — before any filesystem mutation (C5). Unknown dirs and
    /// stopped runs resolve to `None`.
    #[tokio::test]
    async fn find_by_workdir_resolves_registered_run() {
        let (_d, wd) = setup("#!/bin/bash\nexit 0\n").await;
        let engine = HarnessEngine::new();
        assert_eq!(
            engine.find_by_workdir(&wd).await,
            None,
            "nothing registered yet"
        );

        let id = engine.start(wd.clone()).await.unwrap();
        assert_eq!(engine.find_by_workdir(&wd).await, Some(id));
        assert_eq!(
            engine
                .find_by_workdir(Path::new("/nonexistent/elsewhere"))
                .await,
            None,
            "a different workdir must not match"
        );

        // A stopped (removed) run no longer resolves — stale-idle stop +
        // re-register is what makes start-work retries converge.
        engine.stop(id).await.unwrap();
        assert_eq!(engine.find_by_workdir(&wd).await, None);
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
    use std::path::Path;
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
        // Spec 007: the scaffold self-ignores so a fresh worktree's git status
        // isn't polluted by harness runtime state.
        assert_eq!(
            std::fs::read_to_string(wd.join(".agentum-harness/.gitignore")).unwrap(),
            "*\n"
        );
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

    // --- 004 F4: issue → spec.md transform + tracker-stamped planning ---

    #[test]
    fn spec_md_from_issue_preserves_checkboxes() {
        let body = "Intro prose.\n\n- [ ] First criterion\n- [x] Done criterion\n- [ ] Third\n";
        let md = spec_md_from_issue(
            "42",
            "Add widget",
            body,
            "https://github.com/acme/widgets/issues/42",
        );
        assert!(md.starts_with("# Spec 42 — Add widget\n"));
        assert!(md.contains("https://github.com/acme/widgets/issues/42"));
        // Round-trip through the REAL parser: exactly the body's boxes, with
        // checked → Done and unchecked → Pending (no synthesized fallback).
        let list = derive_backlog_from_spec(&md);
        assert_eq!(list.features.len(), 3, "exactly the body's checkboxes");
        assert_eq!(list.features[0].name, "First criterion");
        assert_eq!(list.features[0].state, FeatureState::Pending);
        assert_eq!(list.features[1].name, "Done criterion");
        assert_eq!(list.features[1].state, FeatureState::Done);
        assert_eq!(list.features[2].name, "Third");
        assert!(
            !md.contains("## Acceptance criteria"),
            "no fallback section"
        );
    }

    #[test]
    fn spec_md_from_issue_synthesizes_fallback_ac() {
        let md = spec_md_from_issue(
            "7",
            "Fix the flaky test",
            "Just prose — no checklist anywhere.",
            "https://github.com/acme/widgets/issues/7",
        );
        assert!(md.contains("## Acceptance criteria"));
        let list = derive_backlog_from_spec(&md);
        assert_eq!(list.features.len(), 1, "exactly one synthesized feature");
        assert_eq!(list.features[0].name, "Fix the flaky test");
        assert_eq!(list.features[0].state, FeatureState::Pending);
    }

    #[test]
    fn spec_md_from_issue_strips_control_chars_and_caps() {
        // ESC (C0), a C1 control, DEL — all stripped; \t → two spaces; \n kept.
        let body = "safe\u{1b}[31mred\u{9b}x\u{7f}\tend\nline two";
        let md = spec_md_from_issue("9", "T", body, "https://github.com/a/b/issues/9");
        assert!(!md.contains('\u{1b}'), "ESC stripped");
        assert!(!md.contains('\u{9b}'), "C1 CSI stripped");
        assert!(!md.contains('\u{7f}'), "DEL stripped");
        assert!(md.contains("safe[31mred"), "text around controls survives");
        assert!(md.contains("x  end"), "tab became two spaces");
        assert!(md.contains("line two"), "newlines kept");

        // Oversize body → capped with the marker, and still round-trips.
        let big = "x".repeat(70 * 1024);
        let md = spec_md_from_issue("9", "T", &big, "https://github.com/a/b/issues/9");
        assert!(md.contains("[truncated]"), "cap marker present");
        assert!(
            md.len() < 66 * 1024,
            "body capped near 64 KiB (got {})",
            md.len()
        );
        assert_eq!(
            derive_backlog_from_spec(&md).features.len(),
            1,
            "capped checkbox-free body still gets the fallback AC"
        );
    }

    #[test]
    fn issue_spec_id_is_traversal_proof() {
        // A crafted title cannot escape specs/ — the slug alphabet is [a-z0-9-].
        assert_eq!(issue_spec_id("42", "../../etc/passwd"), "42-etc-passwd");
        assert_eq!(
            issue_spec_id("42", "Add ~/.ssh support!"),
            "42-add-ssh-support"
        );
        // Empty / symbol-only titles fall back to "issue".
        assert_eq!(issue_spec_id("42", ""), "42-issue");
        assert_eq!(issue_spec_id("42", "!!! ///"), "42-issue");
        // The slug caps at 40 chars with no trailing dash.
        let long = issue_spec_id("7", &"very long title ".repeat(10));
        let slug = long.strip_prefix("7-").unwrap();
        assert!(slug.len() <= 40, "slug capped (got {})", slug.len());
        assert!(!slug.ends_with('-'));
        // Case folds; runs of separators collapse to one dash.
        assert_eq!(issue_spec_id("3", "Fix — the   THING"), "3-fix-the-thing");
    }

    #[tokio::test]
    async fn plan_from_spec_with_tracker_stamps_provider_and_url() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let url = "https://github.com/acme/widgets/issues/42";
        let md = spec_md_from_issue("42", "Add widget", "- [ ] A\n- [ ] B\n", url);
        let spec_id = issue_spec_id("42", "Add widget");
        let spec_dir = wd.join(".agentum-harness/specs").join(&spec_id);
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(spec_dir.join("spec.md"), md).unwrap();

        let list = plan_from_spec_with_tracker(wd, &spec_id, "github", url)
            .await
            .unwrap();
        assert_eq!(list.features.len(), 2);
        // Spec 005 F2 (AC 6): every spec-planned backlog records its spec.
        assert_eq!(list.spec_id.as_deref(), Some(spec_id.as_str()));
        // The AC 7 closer: EVERY derived feature carries the issue's provenance
        // (F1's GitHub arm reads slug+number from this URL).
        for f in &list.features {
            assert_eq!(f.tracker_provider.as_deref(), Some("github"));
            assert_eq!(f.tracker_url.as_deref(), Some(url));
        }
        // …and the stamped backlog is what landed on disk.
        let cfg = HarnessConfig::load(wd).await.unwrap();
        assert!(
            cfg.features
                .features
                .iter()
                .all(|f| f.tracker_url.as_deref() == Some(url))
        );
    }

    #[tokio::test]
    async fn plan_from_spec_delegation_unchanged() {
        // The inner refactor must not change plan_from_spec's tracker behavior:
        // no tracker stamping, same derive + persist semantics (the MCP tool
        // path). Spec 005 F2 (C4) deliberately widened one thing: every planner
        // -from-spec — including this one — now stamps `spec_id`.
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let spec_dir = wd.join(".agentum-harness/specs/s1");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(spec_dir.join("spec.md"), "- [ ] A\n- [x] B\n").unwrap();

        let list = plan_from_spec(wd, "s1").await.unwrap();
        assert_eq!(list.features.len(), 2);
        assert_eq!(list.features[1].state, FeatureState::Done);
        assert_eq!(list.spec_id.as_deref(), Some("s1"));
        assert!(!list.roles, "role gates stay off — spec-013 unaffected");
        assert!(
            list.features
                .iter()
                .all(|f| f.tracker_provider.is_none() && f.tracker_url.is_none()),
            "plan_from_spec stamps no tracker provenance"
        );
    }

    /// Spec 005 F1 (AC 2): the post-plan knob write persists `agent_tool`/
    /// `agent_model` while the feature vector, tracker stamps, and the F2
    /// `spec_id` stamp pass through untouched. Reload from disk proves it.
    #[tokio::test]
    async fn update_backlog_knobs_preserves_features_and_writes_knobs() {
        let dir = TempDir::new().unwrap();
        let wd = dir.path();
        let url = "https://github.com/acme/widgets/issues/42";
        let spec_dir = wd.join(".agentum-harness/specs/s1");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(spec_dir.join("spec.md"), "- [ ] A\n- [ ] B\n").unwrap();
        let planned = plan_from_spec_with_tracker(wd, "s1", "github", url)
            .await
            .unwrap();
        assert_eq!(
            planned.agent_tool, "claude",
            "default before the knob write"
        );

        let saved = update_backlog_knobs(wd, |list| {
            list.agent_tool = "codex".into();
            list.agent_model = Some("gpt-5".into());
        })
        .await
        .unwrap();
        assert_eq!(saved.agent_tool, "codex");
        assert_eq!(saved.agent_model.as_deref(), Some("gpt-5"));

        // Reload from disk: knobs persisted, everything else untouched.
        let cfg = HarnessConfig::load(wd).await.unwrap();
        assert_eq!(cfg.features.agent_tool, "codex");
        assert_eq!(cfg.features.agent_model.as_deref(), Some("gpt-5"));
        assert_eq!(
            cfg.features.spec_id.as_deref(),
            Some("s1"),
            "spec_id untouched"
        );
        assert_eq!(cfg.features.features.len(), 2, "feature vector untouched");
        assert!(
            cfg.features.features.iter().all(|f| {
                f.tracker_provider.as_deref() == Some("github")
                    && f.tracker_url.as_deref() == Some(url)
            }),
            "tracker stamps untouched"
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

    /// Spec 006 F3 pin, written BEFORE the brief refresh (AC 7): the rendered
    /// verdict-file contract is **character-identical** — the brief deltas may
    /// change gate checklists, never the wire contract the engine parses.
    #[test]
    fn role_prompt_verdict_contract_is_character_identical() {
        let p = build_role_prompt(RoleKind::Pm, "I", "s", "S", "roles/authoring.json");
        assert!(
            p.contains(
                "=== HOW TO RECORD YOUR VERDICT ===\n\
                 When finished, WRITE your verdict to `roles/authoring.json` (relative to the project root) as exactly this JSON:\n\
                 {\"passed\": true|false, \"summary\": \"one line on what passed or the single most important gap\"}\n\
                 Set passed=false if the gate does not pass. Do not stop until the file is written. Do not ask the human anything."
            ),
            "verdict contract drifted:\n{p}"
        );
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

    /// Spec 006 C1: a tracker-stamped backlog (start-work / spec-from-issue)
    /// yields its shared provenance — the pair Decompose re-stamps through
    /// `plan_from_spec_with_tracker` so the label trail survives roles-on runs.
    #[test]
    fn shared_tracker_provenance_reads_stamped_backlog() {
        let mut list = FeatureList::default();
        list.features.push(Feature {
            id: "F1".into(),
            name: "f1".into(),
            description: String::new(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: Some("github".into()),
            tracker_url: Some("https://github.com/o/r/issues/42".into()),
        });
        assert_eq!(
            shared_tracker_provenance(&list),
            Some((
                "github".to_string(),
                "https://github.com/o/r/issues/42".to_string()
            ))
        );
    }

    /// Spec 006 C1: an unstamped backlog (manual register / MCP plan) has no
    /// provenance — Decompose keeps the tracker-less planner, byte-identical.
    #[test]
    fn shared_tracker_provenance_none_when_unstamped() {
        let mut list = FeatureList::default();
        assert_eq!(shared_tracker_provenance(&list), None);
        list.features.push(Feature {
            id: "F1".into(),
            name: "f1".into(),
            description: String::new(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
            // A provider with no URL is not usable provenance (the GitHub
            // transition arm parses the issue number out of the URL).
            tracker_provider: Some("github".into()),
            tracker_url: None,
        });
        assert_eq!(shared_tracker_provenance(&list), None);
    }
}
