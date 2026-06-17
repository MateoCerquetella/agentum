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

/// Feature state in the harness pipeline. Persisted into `feature_list.json` so
/// a restarted daemon (or the file viewer) sees the live board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureState {
    /// Backlog: not started.
    #[default]
    Pending,
    /// An agent is actively working this feature.
    Coding,
    /// `verify.sh` is running for this feature.
    Verifying,
    /// Verified green — locked in.
    Done,
    /// Exhausted `max_retries` against a red verify; the run halts here.
    Blocked,
}

/// A single feature in the harness backlog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub state: FeatureState,
    /// How many times verification has failed for this feature.
    #[serde(default)]
    pub attempts: u32,
    /// Tail of the last failing `verify.sh` output (surfaced in the UI).
    #[serde(default)]
    pub last_error: Option<String>,
    /// Optional explicit prompt override. When absent the engine derives a
    /// prompt from `AGENTS.md` + this feature's name/description.
    #[serde(default)]
    pub prompt: Option<String>,
}

fn default_max_retries() -> u32 {
    3
}
fn default_agent_tool() -> String {
    "claude".to_string()
}
fn default_settle_grace_secs() -> u64 {
    8
}
fn default_settle_timeout_secs() -> u64 {
    1800
}

/// The `feature_list.json` document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureList {
    pub features: Vec<Feature>,
    /// Verify failures allowed per feature before it is `Blocked`.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Agent CLI to spawn for each feature (`claude`, `codex`, …).
    #[serde(default = "default_agent_tool")]
    pub agent_tool: String,
    /// Optional model passed to the agent CLI.
    #[serde(default)]
    pub agent_model: Option<String>,
    /// Ignore "agent went idle" signals for this many seconds after a prompt is
    /// injected — skips the agent's *initial* idle so we don't verify before it
    /// has done any work.
    #[serde(default = "default_settle_grace_secs")]
    pub settle_grace_secs: u64,
    /// Hard ceiling on how long to wait for an agent to settle before running
    /// the gate anyway.
    #[serde(default = "default_settle_timeout_secs")]
    pub settle_timeout_secs: u64,
}

impl Default for FeatureList {
    fn default() -> Self {
        Self {
            features: Vec::new(),
            max_retries: default_max_retries(),
            agent_tool: default_agent_tool(),
            agent_model: None,
            settle_grace_secs: default_settle_grace_secs(),
            settle_timeout_secs: default_settle_timeout_secs(),
        }
    }
}

/// Overall run state of a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessState {
    /// Loaded but not started.
    #[default]
    Idle,
    /// `init.sh` is running.
    InitVerifying,
    /// Driving features.
    Running,
    /// A `verify.sh` gate is running.
    Verifying,
    /// A feature exhausted its retries — the run halted at the gate.
    Blocked,
    /// All features verified green.
    Done,
    /// `init.sh` failed, or an unrecoverable orchestration error.
    Failed,
}

/// Event emitted by the engine onto its dedicated broadcast bus and streamed to
/// the UI over `WS /api/harness/events`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessEvent {
    StateChanged {
        harness_id: Uuid,
        state: HarnessState,
    },
    FeatureStateChanged {
        harness_id: Uuid,
        feature_id: String,
        state: FeatureState,
    },
    InitStarted {
        harness_id: Uuid,
    },
    InitCompleted {
        harness_id: Uuid,
        success: bool,
        output: String,
    },
    AgentSpawned {
        harness_id: Uuid,
        feature_id: String,
        session_id: Uuid,
    },
    Log {
        harness_id: Uuid,
        feature_id: Option<String>,
        message: String,
    },
    VerifyStarted {
        harness_id: Uuid,
        feature_id: String,
    },
    VerifyCompleted {
        harness_id: Uuid,
        feature_id: String,
        success: bool,
        output: String,
    },
    HandoffWritten {
        harness_id: Uuid,
        feature_id: String,
    },
    HarnessCompleted {
        harness_id: Uuid,
        success: bool,
    },
    Error {
        harness_id: Uuid,
        message: String,
    },
}

/// A live harness run held in the engine's in-memory map.
#[derive(Debug)]
pub struct HarnessRun {
    pub id: Uuid,
    pub workdir: PathBuf,
    pub state: HarnessState,
    pub features: FeatureList,
    pub current_feature: Option<String>,
    pub current_session: Option<Uuid>,
    pub started_at: Instant,
    pub agent_instructions: String,
    /// Set once [`drive`] has been kicked off so the run can't be driven twice.
    pub driving: bool,
}

/// Config loaded from a project's `.harness/` directory.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    pub workdir: PathBuf,
    pub agent_instructions: String,
    pub features: FeatureList,
    pub init_script: Option<PathBuf>,
    pub verify_script: Option<PathBuf>,
}

/// Snapshot of the `.harness/` files for the in-app viewer.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HarnessFiles {
    pub agents_md: Option<String>,
    pub feature_list_json: Option<String>,
    pub init_sh: Option<String>,
    pub verify_sh: Option<String>,
    pub handoff_md: Option<String>,
}

impl HarnessConfig {
    /// Load harness config from a project directory (the parent of `.harness/`).
    pub async fn load(workdir: &Path) -> anyhow::Result<Self> {
        let harness_dir = workdir.join(".harness");
        if !harness_dir.exists() {
            anyhow::bail!("no .harness/ directory found in {}", workdir.display());
        }

        let agents_md = harness_dir.join("AGENTS.md");
        let agent_instructions = if agents_md.exists() {
            tokio::fs::read_to_string(&agents_md).await?
        } else {
            String::new()
        };

        let features_json = harness_dir.join("feature_list.json");
        let features: FeatureList = if features_json.exists() {
            let content = tokio::fs::read_to_string(&features_json).await?;
            serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("feature_list.json is invalid: {e}"))?
        } else {
            anyhow::bail!("no .harness/feature_list.json found");
        };

        let init_script = harness_dir.join("init.sh");
        let init_script = init_script.exists().then_some(init_script);

        let verify_script = harness_dir.join("verify.sh");
        let verify_script = verify_script.exists().then_some(verify_script);

        Ok(Self {
            workdir: workdir.to_path_buf(),
            agent_instructions,
            features,
            init_script,
            verify_script,
        })
    }

    /// Persist the (possibly mutated) feature list back to disk so the board on
    /// disk always matches the live run.
    pub async fn save_features(&self, features: &FeatureList) -> anyhow::Result<()> {
        let path = self.workdir.join(".harness/feature_list.json");
        let content = serde_json::to_string_pretty(features)?;
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    /// Write `handoff.md` after a feature is verified green.
    pub async fn write_handoff(&self, feature: &Feature, output: &str) -> anyhow::Result<()> {
        let path = self.workdir.join(".harness/handoff.md");
        let content = format!(
            "# Handoff — {name}\n\n\
             - **Feature ID:** `{id}`\n\
             - **Status:** {state:?}\n\
             - **Attempts:** {attempts}\n\n\
             ## What was verified\n\n\
             ```\n{output}\n```\n\n\
             ---\n\
             _Written by the Agentum Harness Engine after the verification gate passed._\n",
            name = feature.name,
            id = feature.id,
            state = feature.state,
            attempts = feature.attempts,
            output = output.trim(),
        );
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    /// Read the current `.harness/` file contents for the viewer.
    pub async fn read_files(workdir: &Path) -> HarnessFiles {
        let dir = workdir.join(".harness");
        async fn read(p: PathBuf) -> Option<String> {
            tokio::fs::read_to_string(p).await.ok()
        }
        HarnessFiles {
            agents_md: read(dir.join("AGENTS.md")).await,
            feature_list_json: read(dir.join("feature_list.json")).await,
            init_sh: read(dir.join("init.sh")).await,
            verify_sh: read(dir.join("verify.sh")).await,
            handoff_md: read(dir.join("handoff.md")).await,
        }
    }
}

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
            self.mark_feature_done(harness_id, feature_id, &output)
                .await?;
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

    /// Atomically claim the driver slot. Returns `false` if already driving so
    /// the route can reject a double-run instead of spawning two loops.
    pub async fn claim_driver(&self, harness_id: Uuid) -> anyhow::Result<bool> {
        let run = self.get_run(harness_id).await?;
        let mut r = run.write().await;
        if r.driving && matches!(r.state, HarnessState::Running | HarnessState::Verifying) {
            return Ok(false);
        }
        r.driving = true;
        Ok(true)
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
}

fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    format!("{out}{err}")
}

/// Keep only the last `max` chars of `s` (so a huge verify log doesn't blow up
/// the stored error or the retry prompt we type into the pane).
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    // Snap to a char boundary so we never slice mid-UTF8.
    let start = (start..s.len())
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(s.len());
    format!("…\n{}", &s[start..])
}

/// Build the prompt handed to the agent for one feature: the harness
/// instructions (AGENTS.md) + the scoped feature + the gate contract.
fn build_feature_prompt(instructions: &str, feature: &Feature) -> String {
    if let Some(p) = &feature.prompt {
        return p.clone();
    }
    format!(
        "You are an agent running inside the Agentum Harness Engine.\n\n\
         === HARNESS INSTRUCTIONS (AGENTS.md) ===\n{instructions}\n\n\
         === YOUR CURRENT TASK — EXACTLY ONE FEATURE ===\n\
         Feature: {name}\n\
         ID: {id}\n\
         {desc}\n\n\
         Work ONLY on this feature. When you believe it is complete, stop and \
         wait. The harness will then run the verification gate (verify.sh). If \
         verification fails you will be given the error output and must fix it.",
        instructions = instructions.trim(),
        name = feature.name,
        id = feature.id,
        desc = feature.description,
    )
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
    if let Err(e) = drive_inner(&state, harness_id).await {
        warn!(%harness_id, error = %e, "harness run failed");
        state.harness.emit_error(harness_id, e.to_string());
        let _ = state
            .harness
            .set_state(harness_id, HarnessState::Failed)
            .await;
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

    let workdir = engine.workdir(harness_id).await?;

    // 2. One feature at a time.
    loop {
        let Some(feature) = engine.next_pending_feature(harness_id).await? else {
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

        // 3. Reload config (agent tool/model/timeouts may have been edited).
        let config = HarnessConfig::load(&workdir).await?;
        let session = spawn_feature_agent(state, harness_id, &workdir, &config, &feature).await?;

        // Subscribe to the lifecycle bus BEFORE injecting the prompt so we never
        // miss the working→idle transition for this turn.
        let grace = Duration::from_secs(config.features.settle_grace_secs);
        let timeout = Duration::from_secs(config.features.settle_timeout_secs);

        // 4. Hand the agent its scoped task.
        let prompt = build_feature_prompt(&config.agent_instructions, &feature);
        inject_prompt(state, &session, &prompt).await?;
        engine.log(harness_id, Some(&feature.id), "agent working…");
        wait_for_settle(&state.bus, session.id, grace, timeout).await;

        // 5. Verification gate with retry. A red gate blocks advancement.
        loop {
            engine.log(harness_id, Some(&feature.id), "running verification gate");
            let (passed, output) = engine.run_verify_once(harness_id, &feature.id).await?;
            if passed {
                engine
                    .mark_feature_done(harness_id, &feature.id, &output)
                    .await?;
                engine.log(
                    harness_id,
                    Some(&feature.id),
                    "✓ verify PASSED — feature done",
                );
                break;
            }

            let blocked = engine
                .record_feature_failure(harness_id, &feature.id, &output)
                .await?;
            if blocked {
                engine.set_state(harness_id, HarnessState::Blocked).await?;
                engine.log(
                    harness_id,
                    Some(&feature.id),
                    "✗ verify FAILED — retries exhausted, feature BLOCKED. Run halted.",
                );
                // Leave the agent session alive so the user can intervene.
                return Ok(());
            }

            engine.log(
                harness_id,
                Some(&feature.id),
                "✗ verify FAILED — handing the error back to the agent for a retry",
            );
            engine.set_state(harness_id, HarnessState::Running).await?;
            let retry = format!(
                "The verification gate (verify.sh) FAILED with this output:\n\n{}\n\n\
                 Fix the problem for feature '{}' and stop when done — the gate will run again.",
                tail(&output, 2000),
                feature.name,
            );
            inject_prompt(state, &session, &retry).await?;
            wait_for_settle(&state.bus, session.id, grace, timeout).await;
        }

        // 6. Feature is done — tear down its agent pane before the next one.
        teardown_session(state, &session).await;
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

    let new = NewSession {
        name: name.clone(),
        workdir: workdir.to_string_lossy().into_owned(),
        tool: config.features.agent_tool.clone(),
        model: config.features.agent_model.clone(),
        flags: Vec::new(),
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

/// Type a prompt into the agent's pane after a boot delay (the REPL needs a
/// moment to come up before it will accept input).
async fn inject_prompt(
    state: &AppState,
    session: &agentum_core::Session,
    prompt: &str,
) -> anyhow::Result<()> {
    tokio::time::sleep(AGENT_BOOT_DELAY).await;
    let host = state
        .store
        .get_host(session.host_id.unwrap_or(LOCAL_HOST_ID))
        .await?
        .ok_or_else(|| anyhow::anyhow!("session host missing"))?;
    let target = session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name));
    crate::host_runtime::send_keys(&host, &target, prompt, true)
        .await
        .map_err(|e| anyhow::anyhow!("send_keys failed: {e}"))?;
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
    loop {
        let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
            return; // overall settle timeout — proceed to the gate
        };
        match tokio::time::timeout(remaining, rx.recv()).await {
            Err(_) => return, // timed out waiting
            Ok(Ok(ev)) => {
                if ev.session_id != Some(session_id) {
                    continue;
                }
                match ev.kind.as_str() {
                    "agent.awaiting_input" | "agent.finished" => {
                        // Ignore the agent's *initial* idle before it has had a
                        // chance to act on the prompt.
                        if start.elapsed() >= grace {
                            return;
                        }
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

/// Make a string safe to embed in a tmux session name.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
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
                },
                Feature {
                    id: "feat-2".into(),
                    name: "Feature Two".into(),
                    description: "Second feature".into(),
                    state: FeatureState::Pending,
                    attempts: 0,
                    last_error: None,
                    prompt: None,
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
}
