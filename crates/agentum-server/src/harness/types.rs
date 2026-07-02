//! Harness types: the `.agentum-harness/` contract — feature backlog model
//! (`Feature`/`FeatureList`/`FeatureState`), run/config/event types, the
//! SDD spec-phase + role-gate enums, and the scaffold/migrate/plan/board/
//! decision-log helpers that read and write the on-disk surface. No engine or
//! drive logic lives here — these are the data types + pure file operations.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical per-project harness directory (spec 010). Holds the committable
/// deliverables: `AGENTS.md`, `feature_list.json`, `init.sh`, `verify.sh`,
/// `handoff.md`. Supersedes the legacy `.harness/`.
pub const HARNESS_DIR: &str = ".agentum-harness";
/// Pre-010 directory name. Still read when `.agentum-harness/` is absent so the
/// demo, in-flight worktrees, and the live test keep working — no flag day.
pub const LEGACY_HARNESS_DIR: &str = ".harness";

/// Resolve a project's harness directory: prefer the canonical
/// `.agentum-harness/`, fall back to the legacy `.harness/` when only it exists.
/// Returns the canonical path when neither exists (callers check existence or
/// scaffold).
pub fn resolve_harness_dir(workdir: &Path) -> PathBuf {
    let canonical = workdir.join(HARNESS_DIR);
    if canonical.is_dir() {
        return canonical;
    }
    let legacy = workdir.join(LEGACY_HARNESS_DIR);
    if legacy.is_dir() {
        return legacy;
    }
    canonical
}

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
    /// `verify.sh` (the unit-test gate) is running for this feature.
    Verifying,
    /// The unit-test gate passed; the browser QA gate (`qa.sh` /
    /// browser-verification-loop) is running (spec 012). Maps to the tracker's
    /// "Ready to Test" state.
    ReadyToTest,
    /// Verify passed, but a human confirmation is required (HITL-at-QA, spec
    /// 010c) before the feature is locked in. The run pauses here.
    AwaitingConfirm,
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
    /// Which task tracker this feature mirrors (`board` / `github` / `linear`),
    /// set when the backlog is created from a goal (spec 011/012). Drives the
    /// lifecycle → ticket-state transitions; `None` = no external tracker.
    #[serde(default)]
    pub tracker_provider: Option<String>,
    /// The tracker item's URL, surfaced in the UI (None for the internal board).
    #[serde(default)]
    pub tracker_url: Option<String>,
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
fn default_agent_yolo() -> bool {
    true
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
    /// Run each agent in autonomous (YOLO) mode. The harness is non-interactive:
    /// without this the agent stops at the first permission prompt and never
    /// reaches the gate. We push the canonical Claude marker into the session
    /// flags and let each adapter translate it (see CLAUDE.md "YOLO marker
    /// translation"). Default on — turn it off only for a tool that is safe to
    /// drive without bypass, or for debugging.
    #[serde(default = "default_agent_yolo")]
    pub agent_yolo: bool,
    /// Require ONE human confirmation when `verify.sh` passes, before a feature
    /// is marked `Done` (HITL-at-QA, spec 010c). Off by default = fully
    /// autonomous; on = the run pauses at `AwaitingConfirm` until confirmed.
    #[serde(default)]
    pub hitl_at_qa: bool,
    /// How the browser QA gate runs (spec 012b):
    /// - `Auto` (default): use `qa.sh` if present; else, when browser-verify is
    ///   enabled (`AGENTUM_BROWSER_VERIFY`), spawn a browser-verification-loop
    ///   **agent**; else skip (pass).
    /// - `Script`: always the `qa.sh` shell gate.
    /// - `Agent`: always spawn the QA agent (drives Chrome/Playwright MCP).
    #[serde(default)]
    pub qa_mode: QaMode,
    /// Agent CLI for the QA gate when `qa_mode` spawns one. Defaults to the
    /// feature agent tool; the browser-verification-loop is a Claude skill, so
    /// `claude` is the sensible value.
    #[serde(default)]
    pub qa_agent_tool: Option<String>,
    /// SDD spec id to author + decompose when `roles` is on (spec 013). Points at
    /// `.agentum-harness/specs/<spec_id>/`. `None` = a plain feature run.
    #[serde(default)]
    pub spec_id: Option<String>,
    /// Run the SDD role-gate phases (PM → Architect → … → Reviewer) around the
    /// feature loop (spec 013). Default OFF so existing feature-only backlogs are
    /// driven exactly as before; the SDD intake turns it on.
    #[serde(default)]
    pub roles: bool,
    /// When a role gate exhausts its retries, pause for a human
    /// (`AwaitingConfirm`) instead of halting at `Blocked`. Default OFF = fully
    /// autonomous, including the final review gate (spec 013; supersedes 010's
    /// HITL-at-QA default).
    #[serde(default)]
    pub hitl_on_block: bool,
}

/// How the browser QA gate is executed (spec 012b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QaMode {
    /// `qa.sh` if present, else a QA agent when browser-verify is on, else skip.
    #[default]
    Auto,
    /// Always the `qa.sh` shell gate.
    Script,
    /// Always spawn the browser-verification-loop agent.
    Agent,
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
            agent_yolo: default_agent_yolo(),
            hitl_at_qa: false,
            qa_mode: QaMode::Auto,
            qa_agent_tool: None,
            spec_id: None,
            roles: false,
            hitl_on_block: false,
        }
    }
}

impl FeatureList {
    /// Copy every run knob (everything EXCEPT the `features` vector) from `src`.
    /// Used after `decompose` so deriving a fresh backlog from the spec doesn't
    /// reset the run's configured knobs (`spec_id`/`roles`/`agent_tool`/…) back to
    /// defaults (spec 013). Destructured so a newly-added knob fails to compile
    /// here until it's handled.
    pub(crate) fn copy_knobs_from(&mut self, src: &FeatureList) {
        let FeatureList {
            features: _,
            max_retries,
            agent_tool,
            agent_model,
            settle_grace_secs,
            settle_timeout_secs,
            agent_yolo,
            hitl_at_qa,
            qa_mode,
            qa_agent_tool,
            spec_id,
            roles,
            hitl_on_block,
        } = src.clone();
        self.max_retries = max_retries;
        self.agent_tool = agent_tool;
        self.agent_model = agent_model;
        self.settle_grace_secs = settle_grace_secs;
        self.settle_timeout_secs = settle_timeout_secs;
        self.agent_yolo = agent_yolo;
        self.hitl_at_qa = hitl_at_qa;
        self.qa_mode = qa_mode;
        self.qa_agent_tool = qa_agent_tool;
        self.spec_id = spec_id;
        self.roles = roles;
        self.hitl_on_block = hitl_on_block;
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
    /// A feature passed verify and is awaiting human confirmation (HITL-at-QA);
    /// the run is paused until `POST /{id}/confirm`.
    AwaitingConfirmation,
    /// A feature exhausted its retries — the run halted at the gate.
    Blocked,
    /// All features verified green.
    Done,
    /// `init.sh` failed, or an unrecoverable orchestration error.
    Failed,
}

/// The SDD authoring/role phase a run is in, layered ABOVE the per-feature
/// backlog (spec 013). The existing feature loop is the `Executing` phase; the
/// role gates (`Authoring`/`Architecture`/`Review`) wrap it. Persisted with the
/// run and surfaced in status; every transition is appended to `decisions.md`
/// so it rebuilds on a store-wipe rescan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecPhase {
    /// PM gate: sharpen the spec (acceptance criteria, scope, value).
    #[default]
    Authoring,
    /// Architect gate: validate boundaries, write `architecture.md`.
    Architecture,
    /// Agentless: derive the verify-gated backlog from the spec (`plan_from_spec`).
    Decompose,
    /// The existing feature loop (developer/tester): code → verify → done, WIP=1.
    Executing,
    /// Reviewer gate: final maintainability / completion sign-off.
    Review,
    /// All phases passed.
    Done,
    /// A role gate exhausted its retries — the run halted here.
    Blocked,
    /// A role gate exhausted retries with `hitl_on_block` on — paused for a human.
    AwaitingConfirm,
}

impl SpecPhase {
    /// The role behind this phase's gate, if it is an agent-played gate. `None`
    /// for the agentless (`Decompose`) and terminal phases.
    pub fn role(self) -> Option<RoleKind> {
        match self {
            SpecPhase::Authoring => Some(RoleKind::Pm),
            SpecPhase::Architecture => Some(RoleKind::Architect),
            SpecPhase::Review => Some(RoleKind::Reviewer),
            _ => None,
        }
    }

    /// The next phase once this one's gate passes. Terminal phases return self.
    pub fn advance(self) -> SpecPhase {
        match self {
            SpecPhase::Authoring => SpecPhase::Architecture,
            SpecPhase::Architecture => SpecPhase::Decompose,
            SpecPhase::Decompose => SpecPhase::Executing,
            SpecPhase::Executing => SpecPhase::Review,
            SpecPhase::Review => SpecPhase::Done,
            other => other,
        }
    }

    /// Stable lower-case slug for verdict filenames + `decisions.md` markers.
    pub fn slug(self) -> &'static str {
        match self {
            SpecPhase::Authoring => "authoring",
            SpecPhase::Architecture => "architecture",
            SpecPhase::Decompose => "decompose",
            SpecPhase::Executing => "executing",
            SpecPhase::Review => "review",
            SpecPhase::Done => "done",
            SpecPhase::Blocked => "blocked",
            SpecPhase::AwaitingConfirm => "awaiting_confirm",
        }
    }

    /// Inverse of [`Self::slug`]. `None` for an unrecognized slug.
    pub fn from_slug(slug: &str) -> Option<SpecPhase> {
        Some(match slug {
            "authoring" => SpecPhase::Authoring,
            "architecture" => SpecPhase::Architecture,
            "decompose" => SpecPhase::Decompose,
            "executing" => SpecPhase::Executing,
            "review" => SpecPhase::Review,
            "done" => SpecPhase::Done,
            "blocked" => SpecPhase::Blocked,
            "awaiting_confirm" => SpecPhase::AwaitingConfirm,
            _ => return None,
        })
    }
}

/// Re-derive the current SDD phase from a `decisions.md` body (spec 013): the
/// last `phase: entered <slug>` marker wins. `None` when no phase has been
/// recorded yet (a fresh or pre-013 run). Pure for testability + used to restore
/// a run's phase on a store-wipe rescan.
pub fn rebuild_phase_from_decisions(decisions_md: &str) -> Option<SpecPhase> {
    decisions_md.lines().rev().find_map(|line| {
        let marker = "phase: entered ";
        let idx = line.find(marker)?;
        let rest = &line[idx + marker.len()..];
        let slug = rest.split_whitespace().next()?;
        SpecPhase::from_slug(slug)
    })
}

/// The SDD role behind an agent-played gate (spec 013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleKind {
    Pm,
    Architect,
    Reviewer,
}

impl RoleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RoleKind::Pm => "pm",
            RoleKind::Architect => "architect",
            RoleKind::Reviewer => "reviewer",
        }
    }

    /// The embedded role brief shipped in the binary — central machinery, never
    /// copied per-repo (spec 013; the SDD `ai/roles/*.md` are gitignored and
    /// machine-local, so the harness ships its own gate briefs).
    pub fn brief(self) -> &'static str {
        match self {
            RoleKind::Pm => include_str!("../harness_roles/pm.md"),
            RoleKind::Architect => include_str!("../harness_roles/architect.md"),
            RoleKind::Reviewer => include_str!("../harness_roles/reviewer.md"),
        }
    }
}

/// What to do after a role gate produces a verdict. A pure decision so the
/// autonomy contract — "never prompt a human on a default run" — is unit-testable
/// without spawning an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// Verdict passed — advance to the next phase.
    Advance,
    /// Verdict failed and retries remain — re-run the role agent.
    Retry,
    /// Retries exhausted, autonomous — halt the run at `Blocked`.
    Block,
    /// Retries exhausted, `hitl_on_block` on — pause for a human.
    AwaitConfirm,
}

/// Decide a role gate's outcome from its verdict + attempt budget. `attempt` is
/// the number of attempts ALREADY made (1 after the first run). With
/// `hitl_on_block` off (the default) this NEVER returns `AwaitConfirm`: the run
/// stays fully autonomous, blocking rather than waiting on a human — including at
/// the final review gate (spec 013, supersedes 010's HITL-at-QA default).
pub fn decide_gate(
    passed: bool,
    attempt: u32,
    max_retries: u32,
    hitl_on_block: bool,
) -> GateDecision {
    if passed {
        return GateDecision::Advance;
    }
    if attempt > max_retries {
        if hitl_on_block {
            GateDecision::AwaitConfirm
        } else {
            GateDecision::Block
        }
    } else {
        GateDecision::Retry
    }
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
    /// The run advanced from one SDD phase to another (spec 013).
    PhaseChanged {
        harness_id: Uuid,
        from: SpecPhase,
        to: SpecPhase,
    },
    /// A role gate produced a verdict (spec 013).
    GateResult {
        harness_id: Uuid,
        role: RoleKind,
        passed: bool,
        attempt: u32,
        summary: String,
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
    /// Current SDD phase (spec 013). `Executing` for a plain feature run (so the
    /// status surface is honest even when role gates are off); the role flow
    /// resets it to `Authoring` at the start of [`drive_inner`].
    pub phase: SpecPhase,
    /// How many times the current phase's gate has run (role-gate retry counter).
    pub phase_attempts: u32,
}

/// Config loaded from a project's `.harness/` directory.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    pub workdir: PathBuf,
    /// The resolved harness dir — `.agentum-harness/` or the legacy `.harness/`.
    /// All reads/writes go through this so a project never mixes the two.
    pub harness_dir: PathBuf,
    pub agent_instructions: String,
    pub features: FeatureList,
    pub init_script: Option<PathBuf>,
    pub verify_script: Option<PathBuf>,
    /// The browser QA gate (`qa.sh`), run after `verify.sh` passes (spec 012).
    /// Absent = QA gate skipped (non-web projects aren't blocked on a browser).
    pub qa_script: Option<PathBuf>,
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
        let harness_dir = resolve_harness_dir(workdir);
        if !harness_dir.is_dir() {
            anyhow::bail!(
                "no {HARNESS_DIR}/ (or legacy {LEGACY_HARNESS_DIR}/) directory found in {}",
                workdir.display()
            );
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
            anyhow::bail!("no feature_list.json in {}", harness_dir.display());
        };

        // A run with no features would register and instantly report "done"; a
        // duplicate id would make state writes and the `$HARNESS_FEATURE_ID`
        // gate target the wrong feature. Reject both up front so a bad backlog
        // fails at load instead of misbehaving mid-run.
        if features.features.is_empty() {
            anyhow::bail!("feature_list.json has no features");
        }
        let mut seen = std::collections::HashSet::new();
        for f in &features.features {
            if !seen.insert(f.id.as_str()) {
                anyhow::bail!("duplicate feature id in feature_list.json: {}", f.id);
            }
        }

        let init_script = harness_dir.join("init.sh");
        let init_script = init_script.exists().then_some(init_script);

        let verify_script = harness_dir.join("verify.sh");
        let verify_script = verify_script.exists().then_some(verify_script);

        let qa_script = harness_dir.join("qa.sh");
        let qa_script = qa_script.exists().then_some(qa_script);

        Ok(Self {
            workdir: workdir.to_path_buf(),
            harness_dir,
            agent_instructions,
            features,
            init_script,
            verify_script,
            qa_script,
        })
    }

    /// Persist the (possibly mutated) feature list back to disk so the board on
    /// disk always matches the live run.
    pub async fn save_features(&self, features: &FeatureList) -> anyhow::Result<()> {
        let path = self.harness_dir.join("feature_list.json");
        let content = serde_json::to_string_pretty(features)?;
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    /// Write `handoff.md` after a feature is verified green.
    pub async fn write_handoff(&self, feature: &Feature, output: &str) -> anyhow::Result<()> {
        let path = self.harness_dir.join("handoff.md");
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
        let dir = resolve_harness_dir(workdir);
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

/// Result of scaffolding / migrating a project's harness surface.
#[derive(Debug, Default, Serialize)]
pub struct HarnessScaffold {
    /// Files (relative to workdir) created or written.
    pub written: Vec<String>,
    /// For a migration: where content was sourced (`ai/specs`, `.harness`, or none).
    pub from: Option<String>,
}

/// Scaffold a fresh `.agentum-harness/` skeleton into `workdir` — the only thing
/// agentum writes into a repo (spec 010a). Idempotent: existing files are kept.
pub async fn scaffold_harness(workdir: &Path) -> anyhow::Result<HarnessScaffold> {
    let dir = workdir.join(HARNESS_DIR);
    tokio::fs::create_dir_all(&dir).await?;
    let mut out = HarnessScaffold::default();
    for (name, body) in scaffold_files() {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        tokio::fs::write(&path, body).await?;
        out.written.push(format!("{HARNESS_DIR}/{name}"));
    }
    Ok(out)
}

/// The skeleton file set written by [`scaffold_harness`]. `AGENTS.md` is a small
/// router stub (spec 010 L04); `feature_list.json` seeds one pending feature so
/// the surface loads immediately.
fn scaffold_files() -> Vec<(&'static str, String)> {
    let feature_list = serde_json::to_string_pretty(&FeatureList {
        features: vec![Feature {
            id: "F1".into(),
            name: "First feature".into(),
            description: "Describe one observable behavior; the engine drives it behind verify.sh."
                .into(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: None,
            tracker_url: None,
        }],
        ..FeatureList::default()
    })
    .unwrap_or_else(|_| "{}".into());
    vec![
        (
            "AGENTS.md",
            "# AGENTS\n\n<!-- Router: keep <=200 lines, <=15 hard constraints; link detail into .agentum-harness/docs/*. -->\n\n## Project\n\nTODO: one-paragraph summary.\n\n## Run / Test\n\n- start: `./init.sh`\n- verify: `./verify.sh`\n".to_string(),
        ),
        ("feature_list.json", feature_list),
        (
            "init.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\n# Environment smoke-test: prove the project can build/start. Non-zero aborts the run.\necho \"init: TODO\"\n".to_string(),
        ),
        (
            "verify.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\n# The UNIT-TEST gate. exit 0 = green (advance to QA), non-zero = red (retry/block). $HARNESS_FEATURE_ID names the feature under test. Prefer real end-to-end checks.\necho \"verify: TODO\"\n".to_string(),
        ),
        (
            "qa.sh",
            // The browser QA gate (spec 012): run AFTER verify.sh passes. exit 0 =
            // green → feature Done + ticket Done; non-zero = red → retry like a
            // failed unit gate. A missing qa.sh passes, so non-web projects aren't
            // blocked. For a web surface, drive the browser-verification-loop here.
            "#!/usr/bin/env bash\nset -euo pipefail\n# Browser QA gate — runs after verify.sh is green. $HARNESS_FEATURE_ID names the feature.\n# For a web app, verify the feature in a real browser, e.g. (requires AGENTUM_BROWSER_VERIFY + Playwright MCP):\n#   claude -p \"Use the browser-verification-loop skill to QA feature $HARNESS_FEATURE_ID against the running app. Exit non-zero if any check fails.\"\n# Default: no browser surface to check — pass.\necho \"qa: no browser checks configured — passing\"\n".to_string(),
        ),
    ]
}

/// Migrate a pre-010 project into the unified `.agentum-harness/` surface without
/// hand-rewrite (spec 010a): copies any legacy `.harness/` contract files and any
/// SDD `ai/specs/*` (deliverables only — generic playbooks stay central) into
/// `.agentum-harness/`. Idempotent; never deletes unless `remove_legacy`.
pub async fn migrate_harness(
    workdir: &Path,
    remove_legacy: bool,
) -> anyhow::Result<HarnessScaffold> {
    let dest = workdir.join(HARNESS_DIR);
    tokio::fs::create_dir_all(&dest).await?;
    let mut out = HarnessScaffold::default();

    // 1) legacy .harness/ contract files → .agentum-harness/
    let legacy = workdir.join(LEGACY_HARNESS_DIR);
    if legacy.is_dir() {
        out.from = Some(LEGACY_HARNESS_DIR.to_string());
        copy_dir_contents(&legacy, &dest, &mut out.written, HARNESS_DIR).await?;
        if remove_legacy {
            tokio::fs::remove_dir_all(&legacy).await.ok();
        }
    }

    // 2) SDD ai/specs/* → .agentum-harness/specs/* (deliverables only)
    let ai_specs = workdir.join("ai").join("specs");
    if ai_specs.is_dir() {
        if out.from.is_none() {
            out.from = Some("ai/specs".to_string());
        }
        let specs_dest = dest.join("specs");
        copy_dir_contents(
            &ai_specs,
            &specs_dest,
            &mut out.written,
            &format!("{HARNESS_DIR}/specs"),
        )
        .await?;
    }

    Ok(out)
}

/// Recursively copy the *contents* of `src` into `dst`, recording each written
/// file as `<label>/<relpath>`. Iterative (no async recursion). Existing
/// destination files are overwritten — migration is explicit + idempotent.
async fn copy_dir_contents(
    src: &Path,
    dst: &Path,
    written: &mut Vec<String>,
    label: &str,
) -> anyhow::Result<()> {
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf(), label.to_string())];
    while let Some((from, to, lbl)) = stack.pop() {
        tokio::fs::create_dir_all(&to).await?;
        let mut rd = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = rd.next_entry().await? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            let to_child = to.join(&name);
            if entry.file_type().await?.is_dir() {
                stack.push((entry.path(), to_child, format!("{lbl}/{name_str}")));
            } else {
                tokio::fs::copy(entry.path(), &to_child).await?;
                written.push(format!("{lbl}/{name_str}"));
            }
        }
    }
    Ok(())
}

/// One spec deliverable found under `.agentum-harness/specs/`.
#[derive(Debug, Clone, Serialize)]
pub struct BoardSpec {
    /// Directory name, e.g. `010a-agentum-harness-surface`.
    pub id: String,
    pub has_spec: bool,
    pub has_architecture: bool,
    pub has_tasks: bool,
}

/// A worktree's harness board, reconstructed purely from disk (spec 010b).
#[derive(Debug, Default, Clone, Serialize)]
pub struct HarnessBoard {
    /// The resolved surface dir (`.agentum-harness/` or legacy `.harness/`).
    pub harness_dir: Option<String>,
    /// Spec deliverables under `.agentum-harness/specs/*`.
    pub specs: Vec<BoardSpec>,
    /// The active backlog features (from `feature_list.json`), if present.
    pub features: Vec<Feature>,
}

/// Reconstruct a worktree's board by scanning `.agentum-harness/` **only** — no
/// agentum store / DB consulted. The repo is the durable source of truth; the
/// store is just a rebuildable index. An absent/empty surface yields an empty
/// board (never an error). Pure read — never writes (the mutating lifecycle is
/// 010c).
pub async fn scan_board(workdir: &Path) -> HarnessBoard {
    let dir = resolve_harness_dir(workdir);
    let mut board = HarnessBoard::default();
    if !dir.is_dir() {
        return board;
    }
    board.harness_dir = Some(dir.to_string_lossy().to_string());

    // Active backlog (best-effort: a malformed file leaves features empty).
    if let Ok(content) = tokio::fs::read_to_string(dir.join("feature_list.json")).await {
        if let Ok(list) = serde_json::from_str::<FeatureList>(&content) {
            board.features = list.features;
        }
    }

    // Spec deliverables: any subdir of specs/ is a spec; report which files exist.
    if let Ok(mut rd) = tokio::fs::read_dir(dir.join("specs")).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                let p = entry.path();
                board.specs.push(BoardSpec {
                    id: entry.file_name().to_string_lossy().to_string(),
                    has_spec: p.join("spec.md").exists(),
                    has_architecture: p.join("architecture.md").exists(),
                    has_tasks: p.join("tasks.md").exists(),
                });
            }
        }
        board.specs.sort_by(|a, b| a.id.cmp(&b.id));
    }
    board
}

/// Parse a spec's acceptance-criteria checkboxes into a verify-gated backlog
/// (spec 010c — the SDD→Harness bridge). Each `- [ ]` / `- [x]` line becomes one
/// feature: unchecked → `Pending`, checked → `Done`. Features are numbered `F1..`
/// in document order. Pure/deterministic — no agent call. No checkboxes → empty
/// backlog (the caller decides whether that's an error).
pub fn derive_backlog_from_spec(spec_md: &str) -> FeatureList {
    let mut features = Vec::new();
    for line in spec_md.lines() {
        let t = line.trim_start();
        let (done, rest) = if let Some(r) = t.strip_prefix("- [ ] ") {
            (false, r)
        } else if let Some(r) = t
            .strip_prefix("- [x] ")
            .or_else(|| t.strip_prefix("- [X] "))
        {
            (true, r)
        } else {
            continue;
        };
        let name = rest.trim();
        if name.is_empty() {
            continue;
        }
        let n = features.len() + 1;
        features.push(Feature {
            id: format!("F{n}"),
            name: name.to_string(),
            description: String::new(),
            state: if done {
                FeatureState::Done
            } else {
                FeatureState::Pending
            },
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: None,
            tracker_url: None,
        });
    }
    FeatureList {
        features,
        ..FeatureList::default()
    }
}

/// Build the engine backlog for a spec under `.agentum-harness/specs/<spec_id>/`
/// from its `spec.md` acceptance criteria and write
/// `.agentum-harness/feature_list.json`. Returns the derived list; errors if the
/// spec.md is missing or has no criteria (no silent empty backlog).
pub async fn plan_from_spec(workdir: &Path, spec_id: &str) -> anyhow::Result<FeatureList> {
    plan_from_spec_inner(workdir, spec_id, None).await
}

/// [`plan_from_spec`] + stamp tracker provenance onto every derived feature
/// (spec 004 AC 7): a spec generated from a GitHub issue yields a backlog whose
/// features all carry `tracker_provider`/`tracker_url`, so the harness's
/// existing transition points move the real issue. N features share ONE issue —
/// which is exactly why the GitHub transition arm reads the issue number from
/// the URL, never from the feature id.
pub async fn plan_from_spec_with_tracker(
    workdir: &Path,
    spec_id: &str,
    provider: &str,
    url: &str,
) -> anyhow::Result<FeatureList> {
    plan_from_spec_inner(workdir, spec_id, Some((provider, url))).await
}

/// Shared core: derive the backlog from the spec's checkboxes, optionally stamp
/// tracker provenance, persist `feature_list.json`. The `tracker: None` path is
/// byte-for-byte the pre-004 `plan_from_spec` (the MCP tool is unchanged).
async fn plan_from_spec_inner(
    workdir: &Path,
    spec_id: &str,
    tracker: Option<(&str, &str)>,
) -> anyhow::Result<FeatureList> {
    let dir = workdir.join(HARNESS_DIR);
    let spec_md = dir.join("specs").join(spec_id).join("spec.md");
    let content = tokio::fs::read_to_string(&spec_md)
        .await
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", spec_md.display()))?;
    let mut list = derive_backlog_from_spec(&content);
    if list.features.is_empty() {
        anyhow::bail!(
            "no acceptance-criteria checkboxes (`- [ ]`) found in {}",
            spec_md.display()
        );
    }
    if let Some((provider, url)) = tracker {
        for f in &mut list.features {
            f.tracker_provider = Some(provider.to_string());
            f.tracker_url = Some(url.to_string());
        }
    }
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(
        dir.join("feature_list.json"),
        serde_json::to_string_pretty(&list)?,
    )
    .await?;
    Ok(list)
}

/// Hard cap on the issue body embedded into a generated spec.md (spec 004 F4).
/// Mirrors the UI's snapshot ethos (`GITHUB_ISSUE_BODY_MAX_CHARS`) at a
/// file-appropriate scale: a runaway issue body must not produce an unbounded
/// spec file.
const ISSUE_SPEC_BODY_MAX_BYTES: usize = 64 * 1024;

/// Strip C0/C1 control characters from untrusted issue text so a crafted body
/// cannot smuggle terminal escapes into files/panes. `\n` is kept (structure),
/// `\t` becomes two spaces (mirrors the UI's
/// `escapeLinkedContextControlChars`); `\r` is dropped, which also normalizes
/// CRLF for the checkbox parser.
fn strip_control_chars(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("  "),
            // `is_control` is Unicode Cc: C0 (incl. ESC), DEL, and C1.
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// One-line rendering of an untrusted title: control chars (incl. newlines)
/// collapse into single spaces so the title can sit inside a heading or a
/// `- [ ]` line without breaking either.
fn inline_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_was_space = true;
    for c in title.chars() {
        let mapped = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        if mapped == ' ' {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(mapped);
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

/// Deterministic spec.md from a GitHub issue — no LLM (spec 004 non-goal). The
/// body is verbatim except: C0/C1 control chars stripped (`\n` kept, `\t` →
/// two spaces), capped at 64 KiB with a `[truncated]` marker. When the (capped)
/// body contains no `- [ ]`/`- [x]` checkbox, a fallback
/// "## Acceptance criteria" section with `- [ ] <title>` is appended so
/// [`plan_from_spec`] always round-trips. Checkbox lines stay bare at line
/// start — prefixing them would break [`derive_backlog_from_spec`]'s parse.
pub fn spec_md_from_issue(number: &str, title: &str, body: &str, url: &str) -> String {
    let title = inline_title(title);
    let title = if title.is_empty() {
        format!("Issue {number}")
    } else {
        title
    };

    let mut body = strip_control_chars(body);
    if body.len() > ISSUE_SPEC_BODY_MAX_BYTES {
        let mut end = ISSUE_SPEC_BODY_MAX_BYTES;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
        body.push_str("\n[truncated]");
    }
    let body = body.trim();

    let mut out = format!(
        "# Spec {number} — {title}\n\n\
         > Generated from GitHub issue {url}. The body below is verbatim issue content.\n\n"
    );
    if !body.is_empty() {
        out.push_str(body);
        out.push('\n');
    }
    // Reuse the real parser to decide whether the round-trip would fail — the
    // fallback must trigger exactly when plan_from_spec would bail.
    if derive_backlog_from_spec(&out).features.is_empty() {
        out.push_str(&format!("\n## Acceptance criteria\n\n- [ ] {title}\n"));
    }
    out
}

/// `"<number>-<slug>"` spec directory id for an issue-generated spec. The slug
/// is the title lowercased with `[a-z0-9]+` runs joined by `-`, capped at 40
/// chars, falling back to `"issue"`. Both atoms are server-constructed —
/// `number` is digits-validated by the route and the slug alphabet excludes
/// `/`/`.` — so the `specs/` path join cannot traverse.
pub fn issue_spec_id(number: &str, title: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = true;
    for c in title.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    let mut slug = slug.to_string();
    if slug.len() > 40 {
        slug.truncate(40);
        slug = slug.trim_end_matches('-').to_string();
    }
    if slug.is_empty() {
        slug = "issue".to_string();
    }
    format!("{number}-{slug}")
}

/// One feature to seed into a harness backlog, carrying the tracker provenance
/// so the engine can later drive ticket-state transitions (spec 012). `id` is the
/// tracker's stable handle (board key, issue number, Linear identifier).
#[derive(Debug, Clone)]
pub struct BacklogFeature {
    pub id: String,
    pub name: String,
    pub description: String,
    pub provider: Option<String>,
    pub url: Option<String>,
}

/// Write a harness backlog from an explicit set of features and persist
/// `.agentum-harness/feature_list.json` (spec 011 — chat-to-features).
///
/// Unlike [`plan_from_spec`], the features come from the *task tracker* (the
/// internal board / GitHub / Linear), which is the source of truth; this only
/// derives the harness backlog from them. Every feature starts `Pending` and the
/// harness is left **Idle** — this function never registers or runs anything, so
/// the user reviews the board and explicitly clicks Run (human-gated, per spec).
///
/// `id` becomes the harness feature id — and `$HARNESS_FEATURE_ID` in
/// `verify.sh` — so pass the tracker's stable key. Empty input and duplicate/blank
/// ids are hard errors: a bad backlog fails here instead of misbehaving mid-run,
/// and we never write a silently-empty backlog (mirrors [`plan_from_spec`] / load
/// validation).
pub async fn write_backlog_from_features(
    workdir: &Path,
    features: &[BacklogFeature],
) -> anyhow::Result<FeatureList> {
    if features.is_empty() {
        anyhow::bail!("no features to write to the harness backlog");
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(features.len());
    for bf in features {
        let id = bf.id.trim();
        if id.is_empty() {
            anyhow::bail!("feature id must not be empty");
        }
        if !seen.insert(id.to_string()) {
            anyhow::bail!("duplicate feature id: {id}");
        }
        out.push(Feature {
            id: id.to_string(),
            name: bf.name.trim().to_string(),
            description: bf.description.trim().to_string(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
            tracker_provider: bf.provider.clone(),
            tracker_url: bf.url.clone(),
        });
    }
    let list = FeatureList {
        features: out,
        ..FeatureList::default()
    };
    let dir = workdir.join(HARNESS_DIR);
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(
        dir.join("feature_list.json"),
        serde_json::to_string_pretty(&list)?,
    )
    .await?;
    Ok(list)
}

/// Bootstrap-Contract readiness of a `.agentum-harness/` surface (spec 010d /
/// lectures L03+L06): can-start, can-verify, has instructions, has a backlog.
#[derive(Debug, Default, Clone, Serialize)]
pub struct BootstrapReport {
    pub harness_dir: Option<String>,
    pub agents_md: bool,
    pub init_sh: bool,
    pub verify_sh: bool,
    /// `feature_list.json` parses with ≥1 feature.
    pub backlog: bool,
    /// All of the above → ready to drive.
    pub ready: bool,
}

/// Mechanized cold-start test: scan `.agentum-harness/` and report what's present
/// vs missing. Pure read; an absent surface reports not-ready (no error). Names
/// the gaps so a fresh agent knows exactly what to fix before a run.
pub async fn check_bootstrap(workdir: &Path) -> BootstrapReport {
    let dir = resolve_harness_dir(workdir);
    let mut r = BootstrapReport::default();
    if !dir.is_dir() {
        return r;
    }
    r.harness_dir = Some(dir.to_string_lossy().to_string());
    r.agents_md = dir.join("AGENTS.md").is_file();
    r.init_sh = dir.join("init.sh").is_file();
    r.verify_sh = dir.join("verify.sh").is_file();
    r.backlog = match tokio::fs::read_to_string(dir.join("feature_list.json")).await {
        Ok(content) => serde_json::from_str::<FeatureList>(&content)
            .map(|l| !l.features.is_empty())
            .unwrap_or(false),
        Err(_) => false,
    };
    r.ready = r.agents_md && r.init_sh && r.verify_sh && r.backlog;
    r
}

/// Append one line to the project's **append-only** decision log
/// (`.agentum-harness/decisions.md`, spec 010e / lecture L05) — the durable
/// "why," never overwritten (unlike the lossy rolling `STATE.md`). Creates the
/// surface dir + file on first use.
pub async fn append_decision(workdir: &Path, entry: &str) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let dir = resolve_harness_dir(workdir);
    tokio::fs::create_dir_all(&dir).await?;
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("decisions.md"))
        .await?;
    f.write_all(format!("- {}\n", entry.trim()).as_bytes())
        .await?;
    // Dropping a tokio File does NOT flush — the buffered write lands on the
    // blocking pool later, so two quick appends can reach the OS out of order
    // (surfaced as a Windows CI race in decision_log_is_append_only).
    f.flush().await?;
    Ok(())
}

/// Read the decision log (empty string if there is none).
pub async fn read_decisions(workdir: &Path) -> String {
    let path = resolve_harness_dir(workdir).join("decisions.md");
    tokio::fs::read_to_string(path).await.unwrap_or_default()
}
