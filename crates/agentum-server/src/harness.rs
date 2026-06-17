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
    /// `verify.sh` is running for this feature.
    Verifying,
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
    /// The resolved harness dir — `.agentum-harness/` or the legacy `.harness/`.
    /// All reads/writes go through this so a project never mixes the two.
    pub harness_dir: PathBuf,
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

        Ok(Self {
            workdir: workdir.to_path_buf(),
            harness_dir,
            agent_instructions,
            features,
            init_script,
            verify_script,
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
            "#!/usr/bin/env bash\nset -euo pipefail\n# The gate. exit 0 = green (advance), non-zero = red (retry/block). $HARNESS_FEATURE_ID names the feature under test. Prefer real end-to-end checks.\necho \"verify: TODO\"\n".to_string(),
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
    let dir = workdir.join(HARNESS_DIR);
    let spec_md = dir.join("specs").join(spec_id).join("spec.md");
    let content = tokio::fs::read_to_string(&spec_md)
        .await
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", spec_md.display()))?;
    let list = derive_backlog_from_spec(&content);
    if list.features.is_empty() {
        anyhow::bail!(
            "no acceptance-criteria checkboxes (`- [ ]`) found in {}",
            spec_md.display()
        );
    }
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

    // 2. One feature at a time.
    loop {
        let Some(feature) = engine.next_pending_feature(harness_id).await? else {
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

        // 5. Verification gate with retry. A red gate blocks advancement.
        loop {
            engine.log(harness_id, Some(&feature.id), "running verification gate");
            let (passed, output) = engine.run_verify_once(harness_id, &feature.id).await?;
            if passed {
                if engine.hitl_at_qa(harness_id).await? {
                    // HITL-at-QA: pause for ONE human confirmation before Done.
                    engine.await_confirm(harness_id, &feature.id).await?;
                    engine.log(
                        harness_id,
                        Some(&feature.id),
                        "✓ verify PASSED — awaiting human confirmation (HITL-at-QA). Run paused; POST /{id}/confirm to finalize and resume.",
                    );
                    return Ok(());
                }
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
async fn inject_prompt(
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

    fn feat(id: &str) -> Feature {
        Feature {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            state: FeatureState::Pending,
            attempts: 0,
            last_error: None,
            prompt: None,
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

    // --- 010c slice 2: HITL-at-QA gate ---

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
}
