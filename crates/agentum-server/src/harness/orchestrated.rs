//! Shared-worktree harness orchestration primitives.
//!
//! Agents never write source files directly in this execution mode.  A
//! validated plan establishes disjoint leases, workers receive bounded context
//! packets, and all mutations pass through the journaled patch broker below.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::AppState;
use agentum_core::{HostKind, LOCAL_HOST_ID, NewSession};

pub const EXECUTION_PLAN_VERSION: u32 = 1;
pub const MAX_PACKET_BYTES: usize = 32 * 1024;
pub const MAX_CONCURRENCY: usize = 4;
pub const MAX_PATCH_OPERATIONS: usize = 128;
pub const MAX_PATCH_BYTES: usize = 1024 * 1024;
pub const MISSING_HASH: &str = "missing";

/// Patch application and all verification share this lane, so a build cannot
/// observe half of a multi-file transaction.
static WORKSPACE_GATE: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceCheck {
    pub id: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileReference {
    pub path: String,
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationGate {
    pub command: String,
    #[serde(default)]
    pub acceptance_checks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionTask {
    pub id: String,
    pub objective: String,
    #[serde(default)]
    pub acceptance_checks: Vec<String>,
    #[serde(default)]
    pub writable_files: Vec<String>,
    #[serde(default)]
    pub allowed_create_dirs: Vec<String>,
    #[serde(default)]
    pub read_only: Vec<FileReference>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub contracts: Vec<String>,
    #[serde(default)]
    pub non_goals: Vec<String>,
    pub targeted_gate: VerificationGate,
    #[serde(default)]
    pub integration_task: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPlan {
    pub version: u32,
    pub goal: String,
    pub acceptance_criteria: Vec<AcceptanceCheck>,
    pub tasks: Vec<ExecutionTask>,
    #[serde(default)]
    pub final_gates: Vec<VerificationGate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPacket {
    pub version: u32,
    pub run_id: String,
    pub task_id: String,
    pub goal: String,
    pub objective: String,
    pub acceptance_checks: Vec<AcceptanceCheck>,
    pub architecture: String,
    pub writable_files: Vec<String>,
    pub allowed_create_dirs: Vec<String>,
    pub read_only: Vec<FileReference>,
    pub contracts: Vec<String>,
    pub non_goals: Vec<String>,
    pub dependency_results: HashMap<String, String>,
    pub content_hashes: HashMap<String, String>,
    pub targeted_gate: VerificationGate,
    pub dirty_state_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOperation {
    Create {
        path: String,
        content: String,
    },
    Update {
        path: String,
        expected_hash: String,
        content: String,
    },
    Delete {
        path: String,
        expected_hash: String,
    },
    Rename {
        from: String,
        to: String,
        expected_hash: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSubmission {
    pub run_id: String,
    pub task_id: String,
    pub capability_token: String,
    pub summary: String,
    pub operations: Vec<PatchOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchReceipt {
    pub patch_id: String,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Preimage {
    path: String,
    content: Option<Vec<u8>>,
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn content_hash(path: &Path) -> anyhow::Result<String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(hash_bytes(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MISSING_HASH.to_string()),
        Err(e) => Err(e.into()),
    }
}

fn safe_relative(raw: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(raw);
    if path.is_absolute() || raw.is_empty() {
        anyhow::bail!("path must be a non-empty worktree-relative path: {raw:?}");
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(p) => out.push(p),
            _ => anyhow::bail!("path traversal is not allowed: {raw}"),
        }
    }
    let first = out.components().next().and_then(|c| match c {
        Component::Normal(p) => p.to_str(),
        _ => None,
    });
    if matches!(first, Some(".git" | ".agentum-harness" | ".harness")) {
        anyhow::bail!("harness and git control paths are not writable: {raw}");
    }
    Ok(out)
}

fn check_no_symlink(workdir: &Path, rel: &Path) -> anyhow::Result<()> {
    let mut cursor = workdir.to_path_buf();
    for component in rel.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        cursor.push(part);
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                anyhow::bail!("unsafe symlink in patch path: {}", cursor.display())
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn path_key(path: &Path) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| anyhow::anyhow!("harness paths must be valid UTF-8"))?,
            ),
            _ => anyhow::bail!("path key must be relative and traversal-free"),
        }
    }
    Ok(parts.join("/"))
}

fn files_under(workdir: &Path, raw_dir: &str) -> anyhow::Result<Vec<String>> {
    let rel_dir = safe_relative(raw_dir)?;
    check_no_symlink(workdir, &rel_dir)?;
    let root = workdir.join(&rel_dir);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![root];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                anyhow::bail!(
                    "unsafe symlink in allowed creation directory: {}",
                    entry.path().display()
                );
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(path_key(entry.path().strip_prefix(workdir)?)?);
            }
        }
    }
    Ok(files)
}

fn reaches(tasks: &HashMap<&str, &ExecutionTask>, from: &str, to: &str) -> bool {
    let mut stack = vec![from];
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if id == to {
            return true;
        }
        if let Some(task) = tasks.get(id) {
            stack.extend(task.dependencies.iter().map(String::as_str));
        }
    }
    false
}

fn overlaps(a: &ExecutionTask, b: &ExecutionTask) -> Option<String> {
    for left in &a.writable_files {
        if b.writable_files.contains(left) {
            return Some(left.clone());
        }
        if b.allowed_create_dirs
            .iter()
            .any(|d| Path::new(left).starts_with(d))
        {
            return Some(left.clone());
        }
    }
    for right in &b.writable_files {
        if a.allowed_create_dirs
            .iter()
            .any(|d| Path::new(right).starts_with(d))
        {
            return Some(right.clone());
        }
    }
    for left in &a.allowed_create_dirs {
        for right in &b.allowed_create_dirs {
            if Path::new(left).starts_with(right) || Path::new(right).starts_with(left) {
                return Some(format!("{left} / {right}"));
            }
        }
    }
    None
}

/// Validate the architect-produced artifact before any worker can be spawned.
pub fn validate_plan(plan: &ExecutionPlan, workdir: &Path) -> anyhow::Result<()> {
    if plan.version != EXECUTION_PLAN_VERSION {
        anyhow::bail!("unsupported execution plan version {}", plan.version);
    }
    if plan.tasks.is_empty() {
        anyhow::bail!("execution plan has no tasks");
    }
    let mut acceptance_ids = HashSet::new();
    for check in &plan.acceptance_criteria {
        if check.id.trim().is_empty() || !acceptance_ids.insert(check.id.as_str()) {
            anyhow::bail!(
                "acceptance check ids must be non-empty and unique: {}",
                check.id
            );
        }
    }
    let mut ids = HashSet::new();
    for task in &plan.tasks {
        if task.id.trim().is_empty() || !ids.insert(task.id.as_str()) {
            anyhow::bail!("task ids must be non-empty and unique: {}", task.id);
        }
        for path in task.writable_files.iter().chain(&task.allowed_create_dirs) {
            safe_relative(path)?;
        }
        for dir in &task.allowed_create_dirs {
            let abs = workdir.join(safe_relative(dir)?);
            if abs.exists() && !abs.is_dir() {
                anyhow::bail!("allowed creation path is not a directory: {dir}");
            }
            check_no_symlink(workdir, &safe_relative(dir)?)?;
        }
        if task.targeted_gate.command.trim().is_empty() {
            anyhow::bail!("task {} has an empty targeted gate", task.id);
        }
        for file in &task.read_only {
            let rel = safe_relative(&file.path)?;
            let abs = workdir.join(&rel);
            if !abs.is_file() {
                anyhow::bail!("read-only path does not exist: {}", file.path);
            }
            let body = std::fs::read_to_string(&abs)?;
            for symbol in &file.symbols {
                if !body.contains(symbol) {
                    anyhow::bail!("symbol {symbol:?} does not resolve in {}", file.path);
                }
            }
        }
        for path in &task.writable_files {
            let abs = workdir.join(safe_relative(path)?);
            if !abs.exists()
                && !task
                    .allowed_create_dirs
                    .iter()
                    .any(|d| Path::new(path).starts_with(d))
            {
                anyhow::bail!(
                    "writable path does not exist and is outside an allowed creation directory: {path}"
                );
            }
        }
    }
    let by_id: HashMap<&str, &ExecutionTask> =
        plan.tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    for task in &plan.tasks {
        for dep in &task.dependencies {
            if dep == &task.id || !by_id.contains_key(dep.as_str()) {
                anyhow::bail!("task {} has invalid dependency {dep}", task.id);
            }
            if reaches(&by_id, dep, &task.id) {
                anyhow::bail!(
                    "execution plan dependency cycle includes {} and {dep}",
                    task.id
                );
            }
        }
    }
    let known_ac: HashSet<&str> = plan
        .acceptance_criteria
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    let test_build_only: HashSet<&str> = plan
        .acceptance_criteria
        .iter()
        .filter(|ac| is_test_build_only(&ac.outcome))
        .map(|ac| ac.id.as_str())
        .collect();
    let mut covered = HashSet::new();
    for task in &plan.tasks {
        for ac in &task.acceptance_checks {
            if !known_ac.contains(ac.as_str()) {
                anyhow::bail!("task {} references unknown acceptance check {ac}", task.id);
            }
            if test_build_only.contains(ac.as_str()) {
                anyhow::bail!(
                    "test/build-only acceptance check {ac} must be a final gate, not worker task {}",
                    task.id
                );
            }
            covered.insert(ac.as_str());
        }
    }
    for gate in &plan.final_gates {
        if gate.command.trim().is_empty() {
            anyhow::bail!("final gate command is empty");
        }
        for ac in &gate.acceptance_checks {
            if !known_ac.contains(ac.as_str()) {
                anyhow::bail!("final gate references unknown acceptance check {ac}");
            }
            covered.insert(ac.as_str());
        }
    }
    let missing: Vec<_> = known_ac.difference(&covered).copied().collect();
    if !missing.is_empty() {
        anyhow::bail!("acceptance checks are uncovered: {}", missing.join(", "));
    }
    for (index, a) in plan.tasks.iter().enumerate() {
        for b in &plan.tasks[index + 1..] {
            let ordered = reaches(&by_id, &a.id, &b.id) || reaches(&by_id, &b.id, &a.id);
            if !ordered {
                if let Some(path) = overlaps(a, b) {
                    anyhow::bail!(
                        "concurrent tasks {} and {} have overlapping ownership at {path}",
                        a.id,
                        b.id
                    );
                }
            }
        }
    }
    Ok(())
}

fn is_test_build_only(outcome: &str) -> bool {
    let value = outcome.trim().to_ascii_lowercase();
    [
        "cargo test",
        "cargo check",
        "npm test",
        "npm run test",
        "npm run build",
        "pnpm test",
        "pnpm build",
        "yarn test",
        "yarn build",
        "tests pass",
        "test suite passes",
        "build passes",
        "build succeeds",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn bounded(mut value: String, max: usize) -> String {
    if value.len() <= max {
        return value;
    }
    let mut cut = max.min(value.len());
    while cut > 0 && !value.is_char_boundary(cut) {
        cut -= 1;
    }
    value.truncate(cut);
    value.push_str("\n[truncated]");
    value
}

pub fn compile_packet(
    run_id: &str,
    plan: &ExecutionPlan,
    task: &ExecutionTask,
    workdir: &Path,
    architecture: &str,
    dependency_results: HashMap<String, String>,
    dirty_state_warning: Option<String>,
) -> anyhow::Result<ContextPacket> {
    let accepted: HashSet<&str> = task.acceptance_checks.iter().map(String::as_str).collect();
    let mut hashes = HashMap::new();
    for path in task
        .writable_files
        .iter()
        .chain(task.read_only.iter().map(|r| &r.path))
    {
        hashes.insert(
            path.clone(),
            content_hash(&workdir.join(safe_relative(path)?))?,
        );
    }
    let mut packet = ContextPacket {
        version: EXECUTION_PLAN_VERSION,
        run_id: run_id.to_string(),
        task_id: task.id.clone(),
        goal: plan.goal.clone(),
        objective: task.objective.clone(),
        acceptance_checks: plan
            .acceptance_criteria
            .iter()
            .filter(|a| accepted.contains(a.id.as_str()))
            .cloned()
            .collect(),
        architecture: bounded(architecture.to_string(), 8 * 1024),
        writable_files: task.writable_files.clone(),
        allowed_create_dirs: task.allowed_create_dirs.clone(),
        read_only: task.read_only.clone(),
        contracts: task.contracts.clone(),
        non_goals: task.non_goals.clone(),
        dependency_results: dependency_results
            .into_iter()
            .map(|(k, v)| (k, bounded(v, 2048)))
            .collect(),
        content_hashes: hashes,
        targeted_gate: task.targeted_gate.clone(),
        dirty_state_warning,
    };
    if serde_json::to_vec(&packet)?.len() > MAX_PACKET_BYTES {
        packet.architecture = bounded(packet.architecture, 1024);
        packet.dependency_results.clear();
    }
    if serde_json::to_vec(&packet)?.len() > MAX_PACKET_BYTES {
        anyhow::bail!(
            "context packet for {} exceeds {} bytes",
            task.id,
            MAX_PACKET_BYTES
        );
    }
    Ok(packet)
}

fn capability() -> String {
    hash_bytes(format!("{}:{}", Uuid::new_v4(), Uuid::new_v4()).as_bytes())
}

/// Persist a validated run, immutable packets and initial file leases.
pub async fn initialize_run(
    state: &AppState,
    run_id: Uuid,
    workdir: &Path,
    plan: &ExecutionPlan,
    architecture: &str,
    dirty_warning: Option<String>,
    max_concurrency: usize,
) -> anyhow::Result<String> {
    validate_plan(plan, workdir)?;
    let coordinator_token = capability();
    // Production orchestration is initialized from a registered engine run,
    // which carries the authoritative scope. Keep the historical helper seam
    // usable for local broker tests and callers that initialize directly.
    let scope = match state.harness.workspace(run_id).await {
        Ok(workspace) => workspace.scope().clone(),
        Err(_) => agentum_core::HarnessScope::local_path(workdir.to_string_lossy()),
    };
    state
        .store
        .harness_create_orchestrated_run_scoped(
            &run_id.to_string(),
            &workdir.to_string_lossy(),
            &serde_json::to_string(plan)?,
            &coordinator_token,
            max_concurrency.clamp(1, MAX_CONCURRENCY) as i64,
            &scope,
        )
        .await?;
    for task in &plan.tasks {
        let packet = compile_packet(
            &run_id.to_string(),
            plan,
            task,
            workdir,
            architecture,
            HashMap::new(),
            dirty_warning.clone(),
        )?;
        let ready = if task.dependencies.is_empty() {
            "ready"
        } else {
            "pending"
        };
        let token = capability();
        state
            .store
            .harness_insert_task(
                &run_id.to_string(),
                &task.id,
                None,
                ready,
                &serde_json::to_string(&packet)?,
                &serde_json::to_string(&task.dependencies)?,
                &serde_json::to_string(&task.writable_files)?,
                &serde_json::to_string(&task.allowed_create_dirs)?,
                &token,
                "best_effort",
            )
            .await?;
        let mut leased = HashSet::new();
        for path in &task.writable_files {
            state
                .store
                .harness_insert_lease(
                    &run_id.to_string(),
                    path,
                    &task.id,
                    &content_hash(&workdir.join(safe_relative(path)?))?,
                )
                .await?;
            leased.insert(path.clone());
        }
        for dir in &task.allowed_create_dirs {
            for path in files_under(workdir, dir)? {
                if leased.insert(path.clone()) {
                    state
                        .store
                        .harness_insert_lease(
                            &run_id.to_string(),
                            &path,
                            &task.id,
                            &content_hash(&workdir.join(safe_relative(&path)?))?,
                        )
                        .await?;
                }
            }
        }
    }
    state
        .store
        .harness_update_run(&run_id.to_string(), "ready", None, None)
        .await?;
    Ok(coordinator_token)
}

pub async fn authorize_worker(
    state: &AppState,
    run_id: &str,
    task_id: &str,
    token: &str,
) -> anyhow::Result<agentum_store::harness_orchestration::HarnessOrchestratedTaskRow> {
    let task = state
        .store
        .harness_task(run_id, task_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("unknown harness task {run_id}/{task_id}"))?;
    if task.worker_token != token {
        anyhow::bail!("invalid task capability token");
    }
    Ok(task)
}

pub async fn authorize_coordinator(
    state: &AppState,
    run_id: &str,
    token: &str,
) -> anyhow::Result<agentum_store::harness_orchestration::HarnessOrchestratedRunRow> {
    let run = state
        .store
        .harness_get_orchestrated_run(run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("unknown orchestrated run {run_id}"))?;
    if run.coordinator_token != token {
        anyhow::bail!("invalid coordinator capability token");
    }
    Ok(run)
}

fn op_paths(op: &PatchOperation) -> Vec<&str> {
    match op {
        PatchOperation::Create { path, .. }
        | PatchOperation::Update { path, .. }
        | PatchOperation::Delete { path, .. } => vec![path],
        PatchOperation::Rename { from, to, .. } => vec![from, to],
    }
}

fn expected<'a>(op: &'a PatchOperation, path: &str) -> Option<&'a str> {
    match op {
        PatchOperation::Update { expected_hash, .. }
        | PatchOperation::Delete { expected_hash, .. }
            if op_paths(op)[0] == path =>
        {
            Some(expected_hash)
        }
        PatchOperation::Rename {
            from,
            expected_hash,
            ..
        } if from == path => Some(expected_hash),
        _ => None,
    }
}

fn is_creation_target(op: &PatchOperation, path: &str) -> bool {
    match op {
        PatchOperation::Create { path: target, .. } => target == path,
        PatchOperation::Rename { to, .. } => to == path,
        _ => false,
    }
}

fn atomically_write(path: &Path, content: &[u8], nonce: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".agentum-patch-{nonce}.tmp"));
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn restore(workdir: &Path, preimages: &[Preimage], nonce: &str) {
    for pre in preimages.iter().rev() {
        let Ok(rel) = safe_relative(&pre.path) else {
            continue;
        };
        let path = workdir.join(rel);
        match &pre.content {
            Some(content) => {
                let _ = atomically_write(&path, content, nonce);
            }
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Apply one crash-recoverable multi-file patch transaction.
pub async fn submit_patch(
    state: &AppState,
    submission: PatchSubmission,
) -> anyhow::Result<PatchReceipt> {
    let task = authorize_worker(
        state,
        &submission.run_id,
        &submission.task_id,
        &submission.capability_token,
    )
    .await?;
    if !matches!(
        task.status.as_str(),
        "dispatched" | "working" | "patch_pending"
    ) {
        anyhow::bail!(
            "task {} cannot submit a patch while {}",
            task.task_id,
            task.status
        );
    }
    if submission.operations.is_empty() {
        anyhow::bail!("patch has no operations");
    }
    if submission.operations.len() > MAX_PATCH_OPERATIONS {
        anyhow::bail!(
            "patch has {} operations; maximum is {MAX_PATCH_OPERATIONS}",
            submission.operations.len()
        );
    }
    let patch_bytes = serde_json::to_vec(&submission.operations)?.len();
    if patch_bytes > MAX_PATCH_BYTES {
        anyhow::bail!("patch is {patch_bytes} bytes; maximum is {MAX_PATCH_BYTES}");
    }
    let run = state
        .store
        .harness_get_orchestrated_run(&submission.run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("run disappeared"))?;
    let workdir = PathBuf::from(&run.workdir);
    let writable: HashSet<String> = serde_json::from_str(&task.writable_json)?;
    let create_dirs: Vec<String> = serde_json::from_str(&task.create_dirs_json)?;
    let _lane = WORKSPACE_GATE.lock().await;
    let mut changed = Vec::new();
    let mut preimages = Vec::new();
    let mut touched = HashSet::new();
    for op in &submission.operations {
        for raw in op_paths(op) {
            if !touched.insert(raw) {
                anyhow::bail!("patch touches path more than once: {raw}");
            }
            let rel = safe_relative(raw)?;
            check_no_symlink(&workdir, &rel)?;
            let creation_target =
                is_creation_target(op, raw) && create_dirs.iter().any(|d| rel.starts_with(d));
            let lease = state.store.harness_lease(&submission.run_id, raw).await?;
            let allowed = writable.contains(raw)
                || creation_target
                || lease
                    .as_ref()
                    .is_some_and(|lease| lease.task_id == submission.task_id);
            if !allowed {
                anyhow::bail!("path is outside task ownership: {raw}");
            }
            if let Some(lease) = lease {
                if lease.task_id != submission.task_id {
                    anyhow::bail!("path is leased to task {}: {raw}", lease.task_id);
                }
                if lease.frozen != 0 {
                    anyhow::bail!("lease is frozen after external drift: {raw}");
                }
                let live = content_hash(&workdir.join(&rel))?;
                if live != lease.content_hash {
                    state
                        .store
                        .harness_freeze_lease(&submission.run_id, raw)
                        .await?;
                    state
                        .store
                        .harness_update_task(
                            &submission.run_id,
                            &submission.task_id,
                            "blocked",
                            None,
                            None,
                            Some(&format!("external drift at {raw}")),
                        )
                        .await?;
                    if let Ok(harness_id) = Uuid::parse_str(&submission.run_id) {
                        state.harness.emit(super::HarnessEvent::OwnershipConflict {
                            harness_id,
                            task_id: submission.task_id.clone(),
                            path: raw.to_string(),
                            message: "external edit preserved; lease frozen".into(),
                        });
                    }
                    anyhow::bail!("external drift detected; lease frozen for {raw}");
                }
            } else if !creation_target {
                anyhow::bail!("path has no task lease: {raw}");
            }
            if let Some(want) = expected(op, raw) {
                let live = content_hash(&workdir.join(&rel))?;
                if live != want {
                    anyhow::bail!("stale preimage for {raw}: expected {want}, found {live}");
                }
            }
            let abs = workdir.join(&rel);
            preimages.push(Preimage {
                path: raw.to_string(),
                content: std::fs::read(&abs).ok(),
            });
            changed.push(raw.to_string());
        }
        match op {
            PatchOperation::Create { path, .. }
                if content_hash(&workdir.join(safe_relative(path)?))? != MISSING_HASH =>
            {
                anyhow::bail!("create target already exists: {path}")
            }
            PatchOperation::Update { path, .. } | PatchOperation::Delete { path, .. }
                if content_hash(&workdir.join(safe_relative(path)?))? == MISSING_HASH =>
            {
                anyhow::bail!("patch target does not exist: {path}")
            }
            PatchOperation::Rename { from, to, .. } => {
                if content_hash(&workdir.join(safe_relative(from)?))? == MISSING_HASH {
                    anyhow::bail!("rename source does not exist: {from}");
                }
                if content_hash(&workdir.join(safe_relative(to)?))? != MISSING_HASH {
                    anyhow::bail!("rename target already exists: {to}");
                }
            }
            _ => {}
        }
    }
    let patch_id = Uuid::new_v4().to_string();
    state
        .store
        .harness_update_task(
            &submission.run_id,
            &submission.task_id,
            "patch_pending",
            None,
            None,
            None,
        )
        .await?;
    let operations_json = serde_json::to_string(&submission.operations)?;
    let preimages_json = serde_json::to_string(&preimages)?;
    state
        .store
        .harness_insert_patch(&agentum_store::harness_orchestration::HarnessPatchInsert {
            patch_id: &patch_id,
            run_id: &submission.run_id,
            task_id: &submission.task_id,
            summary: &submission.summary,
            operations_json: &operations_json,
            preimages_json: &preimages_json,
            status: "prepared",
        })
        .await?;
    state
        .store
        .harness_update_patch(&patch_id, "applying", None)
        .await?;
    let applied = (|| -> anyhow::Result<()> {
        for (operation_index, op) in submission.operations.iter().enumerate() {
            #[cfg(test)]
            if submission.summary == "__inject_failure_after_first__" && operation_index == 1 {
                anyhow::bail!("injected patch application failure");
            }
            #[cfg(not(test))]
            let _ = operation_index;
            match op {
                PatchOperation::Create { path, content }
                | PatchOperation::Update { path, content, .. } => {
                    atomically_write(
                        &workdir.join(safe_relative(path)?),
                        content.as_bytes(),
                        &patch_id,
                    )?;
                }
                PatchOperation::Delete { path, .. } => {
                    std::fs::remove_file(workdir.join(safe_relative(path)?))?
                }
                PatchOperation::Rename { from, to, .. } => {
                    let dest = workdir.join(safe_relative(to)?);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::rename(workdir.join(safe_relative(from)?), dest)?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = applied {
        restore(&workdir, &preimages, &patch_id);
        state
            .store
            .harness_update_patch(&patch_id, "rolled_back", Some(&error.to_string()))
            .await?;
        state
            .store
            .harness_update_task(
                &submission.run_id,
                &submission.task_id,
                "working",
                None,
                None,
                Some(&error.to_string()),
            )
            .await?;
        return Err(error);
    }
    for path in &changed {
        if let Some(lease) = state.store.harness_lease(&submission.run_id, path).await? {
            let new_hash = content_hash(&workdir.join(safe_relative(path)?))?;
            state
                .store
                .harness_update_lease_hash(&submission.run_id, path, &new_hash)
                .await?;
            let _ = lease;
        } else {
            let new_hash = content_hash(&workdir.join(safe_relative(path)?))?;
            if new_hash != MISSING_HASH {
                state
                    .store
                    .harness_insert_lease(&submission.run_id, path, &submission.task_id, &new_hash)
                    .await?;
            }
        }
    }
    state
        .store
        .harness_update_patch(&patch_id, "accepted", None)
        .await?;
    state
        .store
        .harness_update_task(
            &submission.run_id,
            &submission.task_id,
            "working",
            None,
            None,
            None,
        )
        .await?;
    Ok(PatchReceipt {
        patch_id,
        changed_paths: changed,
    })
}

/// Roll back transactions interrupted after journaling but before acceptance.
pub async fn recover_incomplete_patches(state: &AppState) -> anyhow::Result<usize> {
    let _lane = WORKSPACE_GATE.lock().await;
    let patches = state.store.harness_incomplete_patches().await?;
    for patch in &patches {
        if let Some(run) = state
            .store
            .harness_get_orchestrated_run(&patch.run_id)
            .await?
        {
            let scope = run.harness_scope()?;
            if let Some(host_id) = scope.host_id.filter(|id| *id != LOCAL_HOST_ID) {
                let remote = state
                    .store
                    .get_host(host_id)
                    .await?
                    .is_none_or(|host| matches!(host.kind, HostKind::Ssh { .. }));
                if remote {
                    // Orchestrated patch application is local-only. Never
                    // interpret an SSH path on the daemon while recovering a
                    // stale/external row, including when its host was deleted.
                    state
                        .store
                        .harness_update_patch(
                            &patch.patch_id,
                            "blocked_remote_mode",
                            Some("remote orchestrated patch recovery is unsupported; filesystem was not touched"),
                        )
                        .await?;
                    continue;
                }
            }
            let workdir = Path::new(&scope.path);
            let preimages: Vec<Preimage> = serde_json::from_str(&patch.preimages_json)?;
            restore(workdir, &preimages, &patch.patch_id);
            for preimage in &preimages {
                if state
                    .store
                    .harness_lease(&patch.run_id, &preimage.path)
                    .await?
                    .is_some()
                {
                    let hash = content_hash(&workdir.join(safe_relative(&preimage.path)?))?;
                    state
                        .store
                        .harness_update_lease_hash(&patch.run_id, &preimage.path, &hash)
                        .await?;
                }
            }
        }
        state
            .store
            .harness_update_patch(
                &patch.patch_id,
                "recovered",
                Some("rolled back during daemon recovery"),
            )
            .await?;
    }
    Ok(patches.len())
}

pub async fn verify_task(
    state: &AppState,
    run_id: &str,
    task_id: &str,
    token: &str,
) -> anyhow::Result<(bool, String)> {
    let task = authorize_worker(state, run_id, task_id, token).await?;
    let packet: ContextPacket = serde_json::from_str(&task.packet_json)?;
    let run = state
        .store
        .harness_get_orchestrated_run(run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("run disappeared"))?;
    let _lane = WORKSPACE_GATE.lock().await;
    // Integrity monitoring catches human/unsupported-CLI writes even when the
    // worker supplies a plausible expected hash. Preserve the edit and freeze.
    let create_dirs: Vec<String> = serde_json::from_str(&task.create_dirs_json)?;
    for dir in create_dirs {
        for path in files_under(Path::new(&run.workdir), &dir)? {
            if state.store.harness_lease(run_id, &path).await?.is_none() {
                state
                    .store
                    .harness_insert_lease(
                        run_id,
                        &path,
                        task_id,
                        &content_hash(&Path::new(&run.workdir).join(safe_relative(&path)?))?,
                    )
                    .await?;
                state.store.harness_freeze_lease(run_id, &path).await?;
                state
                    .store
                    .harness_update_task(
                        run_id,
                        task_id,
                        "blocked",
                        None,
                        None,
                        Some(&format!("unbrokered file creation at {path}")),
                    )
                    .await?;
                state.harness.emit(super::HarnessEvent::OwnershipConflict {
                    harness_id: Uuid::parse_str(run_id)?,
                    task_id: task_id.to_string(),
                    path: path.clone(),
                    message: "external file preserved; lease frozen".into(),
                });
                anyhow::bail!("unbrokered file creation detected; lease frozen for {path}");
            }
        }
    }
    for lease in state
        .store
        .harness_leases(run_id)
        .await?
        .into_iter()
        .filter(|l| l.task_id == task_id)
    {
        let live = content_hash(&Path::new(&run.workdir).join(safe_relative(&lease.path)?))?;
        if live != lease.content_hash {
            state
                .store
                .harness_freeze_lease(run_id, &lease.path)
                .await?;
            state
                .store
                .harness_update_task(
                    run_id,
                    task_id,
                    "blocked",
                    None,
                    None,
                    Some(&format!("external drift at {}", lease.path)),
                )
                .await?;
            anyhow::bail!(
                "external drift detected; preserved edit and froze lease for {}",
                lease.path
            );
        }
    }
    state
        .store
        .harness_update_task(run_id, task_id, "verifying", None, None, None)
        .await?;
    let output = tokio::process::Command::new("bash")
        .args(["-lc", &packet.targeted_gate.command])
        .current_dir(&run.workdir)
        .output()
        .await?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let tail = bounded(text, 4000);
    if output.status.success() {
        state
            .store
            .harness_update_task(
                run_id,
                task_id,
                "completed",
                None,
                Some(&serde_json::json!({"gate":"passed"}).to_string()),
                None,
            )
            .await?;
        state.store.harness_promote_ready_tasks(run_id).await?;
        let all_done = state
            .store
            .harness_tasks(run_id)
            .await?
            .iter()
            .all(|t| t.status == "completed");
        if !all_done
            || !state
                .store
                .harness_increment_final_gate_runs(run_id)
                .await?
        {
            return Ok((true, tail));
        }
        let config = super::HarnessConfig::load(Path::new(&run.workdir)).await?;
        transition_run_tracker(state, &config, crate::task_sink::TrackerPhase::ReadyToTest).await;
        let plan: ExecutionPlan = serde_json::from_str(&run.plan_json)?;
        let mut final_output = String::new();
        for gate in &plan.final_gates {
            let result = tokio::process::Command::new("bash")
                .args(["-lc", &gate.command])
                .current_dir(&run.workdir)
                .output()
                .await?;
            let text = format!(
                "$ {}\n{}{}",
                gate.command,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            final_output.push_str(&bounded(text, 4000));
            final_output.push('\n');
            if !result.status.success() {
                state.store.harness_update_run(run_id, "blocked", None, Some(&serde_json::json!({"final_gate":gate.command,"output_tail":bounded(final_output.clone(),4000)}).to_string())).await?;
                return Ok((false, bounded(final_output, 4000)));
            }
        }
        state
            .store
            .harness_update_run(
                run_id,
                "reviewing",
                None,
                Some(&serde_json::json!({"final_gates":"passed"}).to_string()),
            )
            .await?;
        drop(_lane);
        spawn_reviewer(state, &run).await?;
        Ok((
            true,
            format!(
                "{}\nAll final gates passed; read-only review started.",
                tail
            ),
        ))
    } else {
        state
            .store
            .harness_update_task(run_id, task_id, "working", None, None, Some(&tail))
            .await?;
        Ok((false, tail))
    }
}

async fn spawn_reviewer(
    state: &AppState,
    run: &agentum_store::harness_orchestration::HarnessOrchestratedRunRow,
) -> anyhow::Result<agentum_core::Session> {
    let run_id = Uuid::parse_str(&run.run_id)?;
    let config = super::HarnessConfig::load(Path::new(&run.workdir)).await?;
    let reviewer = spawn_managed(
        state,
        ManagedSpawn {
            run_id,
            task_id: None,
            role: "reviewer",
            workdir: Path::new(&run.workdir),
            tool: &config.features.agent_tool,
            model: config.features.agent_model.as_deref(),
            scope: "reviewer",
        },
    )
    .await?;
    let patches = state.store.harness_patches(&run.run_id).await?;
    let prompt = format!(
        "You are the final read-only reviewer for orchestrated run {}. Review the spec, architecture, \
         accepted patch ledger, and final git diff. Do not edit files and do not read worker transcripts. \
         Final gates already passed. If every product outcome is genuinely met, call \
         agentum_harness_retry_or_block with action=complete and a concise evidence summary; otherwise \
         call it with action=block and the most important gap.\nAccepted patch ledger: {}\n\
         Coordinator/reviewer capability token: {}",
        run.run_id,
        bounded(serde_json::to_string(&patches)?, 12 * 1024),
        run.coordinator_token,
    );
    let _lifecycle = state.bus.subscribe();
    super::inject_prompt(state, &reviewer, &prompt).await?;
    Ok(reviewer)
}

pub async fn enrich_status(
    state: &AppState,
    status: &mut super::HarnessStatus,
) -> anyhow::Result<()> {
    if status.execution_mode != super::ExecutionMode::Orchestrated {
        return Ok(());
    }
    let run_id = status.id.to_string();
    let Some(run) = state.store.harness_get_orchestrated_run(&run_id).await? else {
        return Ok(());
    };
    status.coordinator_session = run
        .coordinator_session
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());
    status.current_session = status.coordinator_session;
    let tasks = state.store.harness_tasks(&run_id).await?;
    let leases = state.store.harness_leases(&run_id).await?;
    let patches = state.store.harness_patches(&run_id).await?;
    status.active_workers = tasks
        .into_iter()
        .filter(|t| {
            matches!(
                t.status.as_str(),
                "dispatched" | "working" | "patch_pending" | "verifying" | "blocked"
            )
        })
        .map(|t| {
            let conflict = leases
                .iter()
                .find(|l| l.task_id == t.task_id && l.frozen != 0)
                .map(|l| format!("external drift at {}", l.path));
            let patch_state = patches
                .iter()
                .rev()
                .find(|p| p.task_id == t.task_id)
                .map(|p| p.status.clone());
            super::OrchestratedWorkerStatus {
                task_id: t.task_id,
                state: t.status,
                session_id: t
                    .worker_session
                    .as_deref()
                    .and_then(|s| Uuid::parse_str(s).ok()),
                enforcement: t.enforcement,
                context_remaining: t.context_remaining,
                patch_state,
                conflict,
            }
        })
        .collect();
    Ok(())
}

pub fn execution_plan_path(workdir: &Path, spec_id: &str) -> PathBuf {
    super::resolve_harness_dir(workdir)
        .join("specs")
        .join(spec_id)
        .join("execution-plan.json")
}

pub fn load_execution_plan(workdir: &Path, spec_id: &str) -> anyhow::Result<ExecutionPlan> {
    let path = execution_plan_path(workdir, spec_id);
    let body = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("architect did not produce {}: {e}", path.display()))?;
    let plan: ExecutionPlan = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("invalid {}: {e}", path.display()))?;
    validate_plan(&plan, workdir)?;
    Ok(plan)
}

fn managed_flags(tool: &str) -> Vec<String> {
    match tool {
        "codex" => vec!["--sandbox".into(), "read-only".into()],
        "claude" => vec!["--permission-mode".into(), "plan".into()],
        _ => Vec::new(),
    }
}

fn enforcement(tool: &str) -> &'static str {
    match tool {
        "codex" | "claude" => "enforced",
        _ => "best_effort",
    }
}

pub async fn transition_run_tracker(
    state: &AppState,
    config: &super::HarnessConfig,
    phase: crate::task_sink::TrackerPhase,
) {
    let Some(feature) = config.features.features.first() else {
        return;
    };
    let Some(provider) = feature.tracker_provider.as_deref() else {
        return;
    };
    let result = crate::task_sink::apply_tracker_transition(
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
    if let Err(error) = result {
        tracing::warn!(%error, ?phase, "orchestrated tracker transition failed (non-fatal)");
    }
}

struct ManagedSpawn<'a> {
    run_id: Uuid,
    task_id: Option<&'a str>,
    role: &'a str,
    workdir: &'a Path,
    tool: &'a str,
    model: Option<&'a str>,
    scope: &'a str,
}

async fn spawn_managed(
    state: &AppState,
    request: ManagedSpawn<'_>,
) -> anyhow::Result<agentum_core::Session> {
    let ManagedSpawn {
        run_id,
        task_id,
        role,
        workdir,
        tool,
        model,
        scope,
    } = request;
    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| anyhow::anyhow!("local host missing"))?;
    let kind = task_id
        .map(|t| format!("worker-{}", super::helpers::sanitize(t)))
        .unwrap_or_else(|| role.to_string());
    let name = super::drive::spawn_session_name(&kind, run_id);
    let session = state
        .store
        .create_session_on_host(
            NewSession {
                name,
                workdir: workdir.to_string_lossy().into_owned(),
                tool: tool.to_string(),
                model: model.map(str::to_owned),
                flags: managed_flags(tool),
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            },
            Some(LOCAL_HOST_ID),
        )
        .await?;
    let target = agentum_tmux::target_for(&session.name);
    crate::routes::sessions::spawn_agent_into_pane(state, &session, &host, &target, workdir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn managed {role}: {e}"))?;
    state
        .store
        .harness_register_managed_session(
            &session.id.to_string(),
            &run_id.to_string(),
            task_id,
            role,
            scope,
        )
        .await?;
    Ok(session)
}

pub async fn spawn_coordinator(
    state: &AppState,
    run_id: Uuid,
    config: &super::HarnessConfig,
    coordinator_token: &str,
) -> anyhow::Result<agentum_core::Session> {
    let session = spawn_managed(
        state,
        ManagedSpawn {
            run_id,
            task_id: None,
            role: "coordinator",
            workdir: &config.workdir,
            tool: &config.features.agent_tool,
            model: config.features.agent_model.as_deref(),
            scope: "coordinator",
        },
    )
    .await?;
    state
        .store
        .harness_update_run(
            &run_id.to_string(),
            "running",
            Some(&session.id.to_string()),
            None,
        )
        .await?;
    state
        .harness
        .set_current_session(run_id, session.id, &session.tool, None)
        .await?;
    let prompt = format!(
        "You are the coordinator for Agentum orchestrated harness run {run_id}.\n\
         Do not edit source files. Use only the agentum_harness_run_state and coordinator harness \
         tools. Dispatch up to {} ready tasks; workers have isolated conversations and immutable \
         packets. Continue until all tasks complete or a server-validated block is unavoidable.\n\
         Coordinator capability token: {coordinator_token}",
        config.features.max_concurrency.min(MAX_CONCURRENCY),
    );
    let _lifecycle = state.bus.subscribe();
    super::inject_prompt(state, &session, &prompt).await?;
    Ok(session)
}

pub async fn dispatch_worker(
    state: &AppState,
    run_id: &str,
    task_id: &str,
    coordinator_token: &str,
) -> anyhow::Result<agentum_core::Session> {
    let run = authorize_coordinator(state, run_id, coordinator_token).await?;
    let task = state
        .store
        .harness_task(run_id, task_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("unknown task {task_id}"))?;
    if task.status != "ready" {
        anyhow::bail!("task {task_id} is {}, not ready", task.status);
    }
    let active = state
        .store
        .harness_tasks(run_id)
        .await?
        .into_iter()
        .filter(|t| {
            matches!(
                t.status.as_str(),
                "dispatched" | "working" | "patch_pending" | "verifying"
            )
        })
        .count();
    if active >= run.max_concurrency.min(MAX_CONCURRENCY as i64) as usize {
        anyhow::bail!("run is at its concurrency ceiling ({active})");
    }
    let config = super::HarnessConfig::load(Path::new(&run.workdir)).await?;
    if active == 0 {
        transition_run_tracker(state, &config, crate::task_sink::TrackerPhase::InProgress).await;
    }
    let scope = format!("task:{task_id}");
    let session = spawn_managed(
        state,
        ManagedSpawn {
            run_id: Uuid::parse_str(run_id)?,
            task_id: Some(task_id),
            role: "worker",
            workdir: Path::new(&run.workdir),
            tool: &config.features.agent_tool,
            model: config.features.agent_model.as_deref(),
            scope: &scope,
        },
    )
    .await?;
    let effective_enforcement = enforcement(&session.tool);
    state
        .store
        .harness_update_task(
            run_id,
            task_id,
            "dispatched",
            Some(&session.id.to_string()),
            None,
            None,
        )
        .await?;
    state
        .store
        .harness_set_task_scope(
            run_id,
            task_id,
            &task.writable_json,
            Some(&session.id.to_string()),
            Some(effective_enforcement),
        )
        .await?;
    let packet: ContextPacket = serde_json::from_str(&task.packet_json)?;
    let prompt = format!(
        "You are an isolated worker for task {task_id} in run {run_id}. You may read the live shared \
         worktree but MUST NOT edit it directly. Retrieve your packet with \
         agentum_harness_task_context, submit changes through agentum_harness_submit_patch, then \
         call agentum_harness_request_verify. If narrowly missing context, name the exact file or \
         symbol; do not rediscover the repository broadly.\nTask capability token: {}\n\
         Enforcement: {effective_enforcement}\nObjective: {}",
        task.worker_token, packet.objective,
    );
    let _lifecycle = state.bus.subscribe();
    super::inject_prompt(state, &session, &prompt).await?;
    state
        .store
        .harness_update_task(run_id, task_id, "working", None, None, None)
        .await?;
    let hid = Uuid::parse_str(run_id)?;
    state.harness.emit(super::HarnessEvent::WorkerChanged {
        harness_id: hid,
        task_id: task_id.to_string(),
        session_id: Some(session.id),
        state: "working".into(),
    });
    state
        .store
        .harness_record_decision(
            run_id,
            "dispatch",
            Some(
                &serde_json::json!({
                    "task_id":task_id,"session_id":session.id,"enforcement":effective_enforcement
                })
                .to_string(),
            ),
        )
        .await?;
    Ok(session)
}

pub async fn task_context(
    state: &AppState,
    run_id: &str,
    task_id: &str,
    token: &str,
    expand_file: Option<&str>,
    symbol: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let task = authorize_worker(state, run_id, task_id, token).await?;
    let packet: ContextPacket = serde_json::from_str(&task.packet_json)?;
    let expansion = if let Some(file) = expand_file {
        let allowed = packet.writable_files.iter().any(|p| p == file)
            || packet.read_only.iter().any(|r| r.path == file);
        if !allowed {
            anyhow::bail!("context expansion is outside the task packet: {file}");
        }
        let run = state
            .store
            .harness_get_orchestrated_run(run_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("run disappeared"))?;
        let body = std::fs::read_to_string(Path::new(&run.workdir).join(safe_relative(file)?))?;
        let selected = if let Some(symbol) = symbol {
            let lines: Vec<&str> = body.lines().collect();
            let hit = lines
                .iter()
                .position(|line| line.contains(symbol))
                .ok_or_else(|| anyhow::anyhow!("symbol {symbol:?} not found in {file}"))?;
            let start = hit.saturating_sub(25);
            lines[start..(hit + 26).min(lines.len())].join("\n")
        } else {
            body
        };
        Some(
            serde_json::json!({"file":file,"symbol":symbol,"content":bounded(selected, 12 * 1024)}),
        )
    } else {
        None
    };
    Ok(serde_json::json!({"packet":packet,"expansion":expansion}))
}

pub async fn run_state(
    state: &AppState,
    run_id: &str,
    token: &str,
) -> anyhow::Result<serde_json::Value> {
    let run = authorize_coordinator(state, run_id, token).await?;
    let tasks = state.store.harness_tasks(run_id).await?;
    let leases = state.store.harness_leases(run_id).await?;
    let patches = state.store.harness_patches(run_id).await?;
    let sessions = state.store.harness_active_sessions(run_id).await?;
    Ok(
        serde_json::json!({"run":run,"tasks":tasks,"leases":leases,"patches":patches,"managed_sessions":sessions}),
    )
}

pub async fn transfer_ownership(
    state: &AppState,
    run_id: &str,
    path: &str,
    from: &str,
    to: &str,
    token: &str,
) -> anyhow::Result<()> {
    authorize_coordinator(state, run_id, token).await?;
    let from_task = state
        .store
        .harness_task(run_id, from)
        .await?
        .ok_or_else(|| anyhow::anyhow!("unknown task {from}"))?;
    let to_task = state
        .store
        .harness_task(run_id, to)
        .await?
        .ok_or_else(|| anyhow::anyhow!("unknown task {to}"))?;
    if matches!(
        from_task.status.as_str(),
        "working" | "patch_pending" | "verifying"
    ) {
        anyhow::bail!("cannot transfer ownership from active task {from}");
    }
    let lease = state
        .store
        .harness_lease(run_id, path)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no lease for {path}"))?;
    if lease.task_id != from {
        anyhow::bail!("lease belongs to {}, not {from}", lease.task_id);
    }
    if !state
        .store
        .harness_transfer_lease(run_id, path, from, to)
        .await?
    {
        anyhow::bail!("lease transfer raced");
    }
    let mut from_files: Vec<String> = serde_json::from_str(&from_task.writable_json)?;
    from_files.retain(|p| p != path);
    let mut to_files: Vec<String> = serde_json::from_str(&to_task.writable_json)?;
    if !to_files.iter().any(|p| p == path) {
        to_files.push(path.to_string());
    }
    state
        .store
        .harness_set_task_scope(
            run_id,
            from,
            &serde_json::to_string(&from_files)?,
            None,
            None,
        )
        .await?;
    state
        .store
        .harness_set_task_scope(run_id, to, &serde_json::to_string(&to_files)?, None, None)
        .await?;
    state
        .store
        .harness_record_decision(
            run_id,
            "transfer_ownership",
            Some(&serde_json::json!({"path":path,"from":from,"to":to}).to_string()),
        )
        .await?;
    Ok(())
}

pub async fn create_repair_task(
    state: &AppState,
    run_id: &str,
    token: &str,
    task: ExecutionTask,
) -> anyhow::Result<()> {
    let run = authorize_coordinator(state, run_id, token).await?;
    let mut plan: ExecutionPlan = serde_json::from_str(&run.plan_json)?;
    if plan.tasks.iter().any(|t| t.id == task.id) {
        anyhow::bail!("task id already exists: {}", task.id);
    }
    plan.tasks.push(task.clone());
    validate_plan(&plan, Path::new(&run.workdir))?;
    for path in &task.writable_files {
        if state.store.harness_lease(run_id, path).await?.is_some() {
            anyhow::bail!(
                "repair task must receive existing ownership through transfer_ownership: {path}"
            );
        }
    }
    let config = super::HarnessConfig::load(Path::new(&run.workdir)).await?;
    let arch = config
        .features
        .spec_id
        .as_deref()
        .map(|id| {
            config
                .harness_dir
                .join("specs")
                .join(id)
                .join("architecture.md")
        })
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let packet = compile_packet(
        run_id,
        &plan,
        &task,
        Path::new(&run.workdir),
        &arch,
        HashMap::new(),
        None,
    )?;
    let existing = state.store.harness_tasks(run_id).await?;
    let completed: HashSet<String> = existing
        .iter()
        .filter(|t| t.status == "completed")
        .map(|t| t.task_id.clone())
        .collect();
    let status = if task.dependencies.iter().all(|d| completed.contains(d)) {
        "ready"
    } else {
        "pending"
    };
    state
        .store
        .harness_insert_task(
            run_id,
            &task.id,
            None,
            status,
            &serde_json::to_string(&packet)?,
            &serde_json::to_string(&task.dependencies)?,
            &serde_json::to_string(&task.writable_files)?,
            &serde_json::to_string(&task.allowed_create_dirs)?,
            &capability(),
            "best_effort",
        )
        .await?;
    for path in &task.writable_files {
        state
            .store
            .harness_insert_lease(
                run_id,
                path,
                &task.id,
                &content_hash(&Path::new(&run.workdir).join(safe_relative(path)?))?,
            )
            .await?;
    }
    state
        .store
        .harness_replace_plan(run_id, &serde_json::to_string(&plan)?)
        .await?;
    state
        .store
        .harness_record_decision(
            run_id,
            "create_repair_task",
            Some(&serde_json::json!({"task_id":task.id}).to_string()),
        )
        .await?;
    Ok(())
}

pub async fn rotate_managed_session(state: &AppState, old_id: Uuid) -> anyhow::Result<Uuid> {
    if !state
        .store
        .harness_claim_session_rotation(&old_id.to_string())
        .await?
    {
        anyhow::bail!("managed session is already rotating or inactive");
    }
    match rotate_managed_session_claimed(state, old_id).await {
        Ok(id) => Ok(id),
        Err(error) => {
            let _ = state
                .store
                .harness_cancel_session_rotation(&old_id.to_string())
                .await;
            Err(error)
        }
    }
}

async fn rotate_managed_session_claimed(state: &AppState, old_id: Uuid) -> anyhow::Result<Uuid> {
    let managed = state
        .store
        .harness_managed_session(&old_id.to_string())
        .await?
        .filter(|m| m.active == 2)
        .ok_or_else(|| anyhow::anyhow!("session is not claimed for rotation"))?;
    let run = state
        .store
        .harness_get_orchestrated_run(&managed.run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("managed run disappeared"))?;
    if !run_status_allows_managed_activity(&run.status) {
        anyhow::bail!("managed run is not active: {}", run.status);
    }
    let config = super::HarnessConfig::load(Path::new(&run.workdir)).await?;
    let checkpoint = run_state(state, &managed.run_id, &run.coordinator_token).await?;
    state
        .store
        .harness_update_run(
            &managed.run_id,
            "rotating",
            None,
            Some(&checkpoint.to_string()),
        )
        .await?;
    let replacement = spawn_managed(
        state,
        ManagedSpawn {
            run_id: Uuid::parse_str(&managed.run_id)?,
            task_id: managed.task_id.as_deref(),
            role: &managed.role,
            workdir: Path::new(&run.workdir),
            tool: &config.features.agent_tool,
            model: config.features.agent_model.as_deref(),
            scope: &managed.capability_scope,
        },
    )
    .await?;
    let prompt = if managed.role == "coordinator" {
        state
            .store
            .harness_update_run(
                &managed.run_id,
                "running",
                Some(&replacement.id.to_string()),
                Some(&checkpoint.to_string()),
            )
            .await?;
        state
            .harness
            .set_current_session(
                Uuid::parse_str(&managed.run_id)?,
                replacement.id,
                &replacement.tool,
                None,
            )
            .await?;
        format!(
            "Resume coordinator run {} from this durable bounded checkpoint. Do not request worker transcripts.\nCheckpoint: {}\nCoordinator capability token: {}",
            managed.run_id,
            bounded(checkpoint.to_string(), 16 * 1024),
            run.coordinator_token
        )
    } else {
        let task_id = managed
            .task_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("worker has no task id"))?;
        let task = state
            .store
            .harness_task(&managed.run_id, task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("task disappeared"))?;
        state
            .store
            .harness_update_task(
                &managed.run_id,
                task_id,
                "working",
                Some(&replacement.id.to_string()),
                None,
                None,
            )
            .await?;
        format!(
            "Resume isolated task {task_id} for run {} from its immutable packet. Retrieve it with agentum_harness_task_context; do not ask for another worker transcript.\nTask capability token: {}",
            managed.run_id, task.worker_token
        )
    };
    let _lifecycle = state.bus.subscribe();
    super::inject_prompt(state, &replacement, &prompt).await?;
    state
        .store
        .harness_replace_managed_session(&old_id.to_string(), &replacement.id.to_string())
        .await?;
    if let Some(old) = state.store.get_session_by_id(old_id).await? {
        super::teardown_session(state, &old).await;
    }
    state.harness.emit(super::HarnessEvent::CoordinatorRotated {
        harness_id: Uuid::parse_str(&managed.run_id)?,
        previous_session: old_id,
        replacement_session: replacement.id,
    });
    Ok(replacement.id)
}

/// Boot recovery: finish/roll back journals first, then freeze any lease whose
/// durable hash no longer matches the shared worktree. External edits are
/// preserved; resumption waits for coordinator resolution.
const RECOVERY_QUARANTINED: &str = "recovery_quarantined";

pub(crate) fn run_status_allows_managed_activity(status: &str) -> bool {
    matches!(status, "running" | "reviewing" | "final_verifying")
}

async fn quarantine_recovery(
    state: &AppState,
    run_id: &str,
    reason: impl Into<String>,
) -> anyhow::Result<()> {
    let reason = reason.into();
    let checkpoint = serde_json::json!({
        "recovery_quarantine": {
            "reason": reason,
            "filesystem_touched": false
        }
    })
    .to_string();
    state
        .store
        .harness_update_run(run_id, RECOVERY_QUARANTINED, None, Some(&checkpoint))
        .await?;
    tracing::warn!(%run_id, %reason, "quarantined harness recovery");
    Ok(())
}

pub async fn recover_orchestrated_runs(state: &AppState) -> anyhow::Result<()> {
    recover_incomplete_patches(state).await?;
    for run in state.store.harness_orchestrated_runs().await? {
        if matches!(
            run.status.as_str(),
            "completed" | "stopped" | RECOVERY_QUARANTINED | "blocked_remote_mode"
        ) {
            continue;
        }
        let scope = match run.harness_scope() {
            Ok(scope) => scope,
            Err(error) => {
                quarantine_recovery(
                    state,
                    &run.run_id,
                    format!("stored harness scope is invalid: {error}"),
                )
                .await?;
                continue;
            }
        };

        // Every non-nil host id is a registered remote binding. Orchestrated
        // recovery is local-only, so quarantine before loading config, probing
        // the host, or touching `scope.path`. This is intentionally identical
        // for an online, offline, retargeted, or deleted SSH host: none may
        // reinterpret its path on the daemon, and none may prevent startup.
        if let Some(host_id) = scope.host_id.filter(|id| *id != LOCAL_HOST_ID) {
            let reason = match state.store.get_host(host_id).await? {
                None => format!("remote harness host was deleted: {host_id}"),
                Some(host) if matches!(host.kind, HostKind::Ssh { .. }) => format!(
                    "remote orchestrated recovery is unsupported for host {host_id}; the host may be offline or stale"
                ),
                Some(_) => format!("remote harness host binding changed kind under id {host_id}"),
            };
            quarantine_recovery(state, &run.run_id, reason).await?;
            continue;
        }

        let host = match scope.host_id {
            Some(host_id) => match state.store.get_host(host_id).await? {
                Some(host) if matches!(host.kind, HostKind::Local) => Some(host),
                Some(_) => {
                    quarantine_recovery(
                        state,
                        &run.run_id,
                        format!("local harness scope resolved to a remote host: {host_id}"),
                    )
                    .await?;
                    continue;
                }
                None => {
                    quarantine_recovery(
                        state,
                        &run.run_id,
                        format!("local harness host is missing: {host_id}"),
                    )
                    .await?;
                    continue;
                }
            },
            None => None,
        };
        let scoped_workdir = Path::new(&scope.path);
        let mut conflict = false;
        for lease in state.store.harness_leases(&run.run_id).await? {
            let live = content_hash(&scoped_workdir.join(safe_relative(&lease.path)?))?;
            if live != lease.content_hash {
                conflict = true;
                state
                    .store
                    .harness_freeze_lease(&run.run_id, &lease.path)
                    .await?;
            }
        }
        for task in state.store.harness_tasks(&run.run_id).await? {
            let create_dirs: Vec<String> = serde_json::from_str(&task.create_dirs_json)?;
            for dir in create_dirs {
                for path in files_under(scoped_workdir, &dir)? {
                    if state
                        .store
                        .harness_lease(&run.run_id, &path)
                        .await?
                        .is_none()
                    {
                        conflict = true;
                        state
                            .store
                            .harness_insert_lease(
                                &run.run_id,
                                &path,
                                &task.task_id,
                                &content_hash(&scoped_workdir.join(safe_relative(&path)?))?,
                            )
                            .await?;
                        state.store.harness_freeze_lease(&run.run_id, &path).await?;
                        state
                            .store
                            .harness_update_task(
                                &run.run_id,
                                &task.task_id,
                                "blocked",
                                None,
                                None,
                                Some(&format!("unbrokered file creation at {path}")),
                            )
                            .await?;
                    }
                }
            }
        }
        if conflict {
            state
                .store
                .harness_update_run(&run.run_id, "recovery_conflict", None, None)
                .await?;
        }
        let harness_id = Uuid::parse_str(&run.run_id)?;
        let coordinator_session = run
            .coordinator_session
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok());
        state
            .harness
            .restore_orchestrated(
                harness_id,
                scope,
                host,
                if conflict {
                    super::HarnessState::Blocked
                } else {
                    super::HarnessState::Running
                },
                coordinator_session,
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, deps: &[&str], files: &[&str]) -> ExecutionTask {
        ExecutionTask {
            id: id.into(),
            objective: id.into(),
            acceptance_checks: vec![id.into()],
            writable_files: files.iter().map(|s| s.to_string()).collect(),
            allowed_create_dirs: vec![],
            read_only: vec![],
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            contracts: vec![],
            non_goals: vec![],
            targeted_gate: VerificationGate {
                command: "true".into(),
                acceptance_checks: vec![],
            },
            integration_task: false,
        }
    }

    #[test]
    fn validates_coverage_cycles_and_concurrent_ownership() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "a").unwrap();
        let good = ExecutionPlan {
            version: 1,
            goal: "g".into(),
            acceptance_criteria: vec![
                AcceptanceCheck {
                    id: "a".into(),
                    outcome: "a".into(),
                },
                AcceptanceCheck {
                    id: "b".into(),
                    outcome: "b".into(),
                },
            ],
            tasks: vec![task("a", &[], &["a"]), task("b", &["a"], &["a"])],
            final_gates: vec![],
        };
        validate_plan(&good, dir.path()).unwrap();
        let mut overlap = good.clone();
        overlap.tasks[1].dependencies.clear();
        assert!(
            validate_plan(&overlap, dir.path())
                .unwrap_err()
                .to_string()
                .contains("overlapping")
        );
        let mut cycle = good.clone();
        cycle.tasks[0].dependencies.push("b".into());
        assert!(
            validate_plan(&cycle, dir.path())
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }

    #[test]
    fn rejects_control_paths_and_caps_packets() {
        assert!(safe_relative("../x").is_err());
        assert!(safe_relative(".git/config").is_err());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "a").unwrap();
        let plan = ExecutionPlan {
            version: 1,
            goal: "g".into(),
            acceptance_criteria: vec![AcceptanceCheck {
                id: "a".into(),
                outcome: "a".into(),
            }],
            tasks: vec![task("a", &[], &["a"])],
            final_gates: vec![],
        };
        let packet = compile_packet(
            "r",
            &plan,
            &plan.tasks[0],
            dir.path(),
            &"x".repeat(50_000),
            HashMap::new(),
            None,
        )
        .unwrap();
        assert!(serde_json::to_vec(&packet).unwrap().len() <= MAX_PACKET_BYTES);
    }

    #[test]
    fn extracts_test_only_acceptance_checks_to_final_gates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "a").unwrap();
        let mut plan = ExecutionPlan {
            version: 1,
            goal: "g".into(),
            acceptance_criteria: vec![AcceptanceCheck {
                id: "tests".into(),
                outcome: "cargo test --workspace passes".into(),
            }],
            tasks: vec![task("tests", &[], &["a"])],
            final_gates: vec![VerificationGate {
                command: "cargo test --workspace".into(),
                acceptance_checks: vec!["tests".into()],
            }],
        };
        assert!(
            validate_plan(&plan, dir.path())
                .unwrap_err()
                .to_string()
                .contains("final gate")
        );
        plan.tasks[0].acceptance_checks.clear();
        validate_plan(&plan, dir.path()).unwrap();
    }

    async fn broker_fixture() -> (tempfile::TempDir, AppState, Uuid, ExecutionPlan) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a0").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b0").unwrap();
        std::fs::create_dir(dir.path().join("new")).unwrap();
        let mut a = task("a", &[], &["a.txt"]);
        a.allowed_create_dirs = vec!["new".into()];
        let b = task("b", &[], &["b.txt"]);
        let plan = ExecutionPlan {
            version: 1,
            goal: "broker".into(),
            acceptance_criteria: vec![
                AcceptanceCheck {
                    id: "a".into(),
                    outcome: "a".into(),
                },
                AcceptanceCheck {
                    id: "b".into(),
                    outcome: "b".into(),
                },
            ],
            tasks: vec![a, b],
            final_gates: vec![],
        };
        let store = agentum_store::Store::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        let (bus, _) = tokio::sync::broadcast::channel(16);
        let state = AppState::new(store, bus);
        let run = Uuid::new_v4();
        initialize_run(&state, run, dir.path(), &plan, "architecture", None, 4)
            .await
            .unwrap();
        for id in ["a", "b"] {
            state
                .store
                .harness_update_task(&run.to_string(), id, "working", None, None, None)
                .await
                .unwrap();
        }
        (dir, state, run, plan)
    }

    #[tokio::test]
    async fn remote_recovery_quarantines_offline_and_deleted_hosts_without_local_io() {
        let dir = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&dir.path().join("recovery.sqlite"))
            .await
            .unwrap();
        let (bus, _) = tokio::sync::broadcast::channel(16);
        let state = AppState::new(store, bus);
        let existing = state
            .store
            .create_host(agentum_core::NewHost {
                name: "offline".into(),
                kind: HostKind::Ssh {
                    user: "dev".into(),
                    hostname: "203.0.113.254".into(),
                    port: 22,
                    auth: agentum_core::SshAuth::Agent,
                },
            })
            .await
            .unwrap();

        for (run_id, host_id) in [
            (Uuid::new_v4(), existing.id),
            (Uuid::new_v4(), Uuid::new_v4()),
        ] {
            let scope = agentum_core::HarnessScope {
                worktree_id: Some(format!("repo::/remote/{run_id}")),
                repo_id: Some("repo".into()),
                host_id: Some(host_id),
                // If recovery ever falls back to local, this same-looking path
                // would be touched. Quarantine happens before any path access.
                path: format!("/remote/{run_id}"),
            };
            state
                .store
                .harness_create_orchestrated_run_scoped(
                    &run_id.to_string(),
                    &scope.path,
                    "{}",
                    "token",
                    4,
                    &scope,
                )
                .await
                .unwrap();
        }

        recover_orchestrated_runs(&state).await.unwrap();
        // A second boot is idempotent: quarantined rows stay inert.
        recover_orchestrated_runs(&state).await.unwrap();
        for row in state.store.harness_orchestrated_runs().await.unwrap() {
            assert_eq!(row.status, RECOVERY_QUARANTINED);
            assert!(
                row.checkpoint_json
                    .as_deref()
                    .unwrap_or_default()
                    .contains("filesystem_touched")
            );
        }
        assert!(state.harness.list().await.is_empty());
    }

    #[test]
    fn quarantined_and_blocked_runs_cannot_revive_managed_workers() {
        assert!(run_status_allows_managed_activity("running"));
        assert!(run_status_allows_managed_activity("reviewing"));
        assert!(!run_status_allows_managed_activity(RECOVERY_QUARANTINED));
        assert!(!run_status_allows_managed_activity("blocked_remote_mode"));
        assert!(!run_status_allows_managed_activity("recovery_conflict"));
    }

    #[tokio::test]
    async fn broker_serializes_disjoint_workers_and_rejects_stale_or_traversal() {
        let (dir, state, run, _) = broker_fixture().await;
        let ta = state
            .store
            .harness_task(&run.to_string(), "a")
            .await
            .unwrap()
            .unwrap();
        let tb = state
            .store
            .harness_task(&run.to_string(), "b")
            .await
            .unwrap()
            .unwrap();
        let pa = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: ta.worker_token.clone(),
            summary: "a".into(),
            operations: vec![PatchOperation::Update {
                path: "a.txt".into(),
                expected_hash: content_hash(&dir.path().join("a.txt")).unwrap(),
                content: "a1".into(),
            }],
        };
        let pb = PatchSubmission {
            run_id: run.to_string(),
            task_id: "b".into(),
            capability_token: tb.worker_token,
            summary: "b".into(),
            operations: vec![PatchOperation::Update {
                path: "b.txt".into(),
                expected_hash: content_hash(&dir.path().join("b.txt")).unwrap(),
                content: "b1".into(),
            }],
        };
        let (ra, rb) = tokio::join!(submit_patch(&state, pa), submit_patch(&state, pb));
        assert!(ra.is_ok() && rb.is_ok());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "a1"
        );
        let stale = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: ta.worker_token.clone(),
            summary: "stale".into(),
            operations: vec![PatchOperation::Update {
                path: "a.txt".into(),
                expected_hash: hash_bytes(b"a0"),
                content: "bad".into(),
            }],
        };
        assert!(
            submit_patch(&state, stale)
                .await
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        let traversal = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: ta.worker_token,
            summary: "escape".into(),
            operations: vec![PatchOperation::Create {
                path: "../escape".into(),
                content: "bad".into(),
            }],
        };
        assert!(submit_patch(&state, traversal).await.is_err());
    }

    #[tokio::test]
    async fn broker_rolls_back_multi_file_failure_and_freezes_external_drift() {
        let (dir, state, run, _) = broker_fixture().await;
        let task = state
            .store
            .harness_task(&run.to_string(), "a")
            .await
            .unwrap()
            .unwrap();
        let fail = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: task.worker_token.clone(),
            summary: "__inject_failure_after_first__".into(),
            operations: vec![
                PatchOperation::Update {
                    path: "a.txt".into(),
                    expected_hash: content_hash(&dir.path().join("a.txt")).unwrap(),
                    content: "temporary".into(),
                },
                PatchOperation::Create {
                    path: "new/created.txt".into(),
                    content: "created".into(),
                },
            ],
        };
        assert!(submit_patch(&state, fail).await.is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "a0"
        );
        assert!(!dir.path().join("new/created.txt").exists());

        let create = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: task.worker_token.clone(),
            summary: "create".into(),
            operations: vec![PatchOperation::Create {
                path: "new/created.txt".into(),
                content: "created".into(),
            }],
        };
        submit_patch(&state, create).await.unwrap();
        let rename = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: task.worker_token.clone(),
            summary: "rename".into(),
            operations: vec![PatchOperation::Rename {
                from: "new/created.txt".into(),
                to: "new/renamed.txt".into(),
                expected_hash: hash_bytes(b"created"),
            }],
        };
        submit_patch(&state, rename).await.unwrap();
        let delete = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: task.worker_token.clone(),
            summary: "delete".into(),
            operations: vec![PatchOperation::Delete {
                path: "new/renamed.txt".into(),
                expected_hash: hash_bytes(b"created"),
            }],
        };
        submit_patch(&state, delete).await.unwrap();
        assert!(!dir.path().join("new/renamed.txt").exists());

        std::fs::write(dir.path().join("a.txt"), "human edit").unwrap();
        let drift = PatchSubmission {
            run_id: run.to_string(),
            task_id: "a".into(),
            capability_token: task.worker_token,
            summary: "drift".into(),
            operations: vec![PatchOperation::Update {
                path: "a.txt".into(),
                expected_hash: hash_bytes(b"human edit"),
                content: "worker".into(),
            }],
        };
        assert!(
            submit_patch(&state, drift)
                .await
                .unwrap_err()
                .to_string()
                .contains("drift")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "human edit"
        );
        assert_eq!(
            state
                .store
                .harness_lease(&run.to_string(), "a.txt")
                .await
                .unwrap()
                .unwrap()
                .frozen,
            1
        );
    }

    #[tokio::test]
    async fn verification_freezes_unbrokered_file_creation() {
        let (dir, state, run, _) = broker_fixture().await;
        let task = state
            .store
            .harness_task(&run.to_string(), "a")
            .await
            .unwrap()
            .unwrap();
        std::fs::write(dir.path().join("new/human.txt"), "preserve me").unwrap();
        let error = verify_task(&state, &run.to_string(), "a", &task.worker_token)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unbrokered file creation"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new/human.txt")).unwrap(),
            "preserve me"
        );
        assert_eq!(
            state
                .store
                .harness_lease(&run.to_string(), "new/human.txt")
                .await
                .unwrap()
                .unwrap()
                .frozen,
            1
        );
    }
}
