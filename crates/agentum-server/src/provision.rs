//! Workspace provisioning — "born ready" (spec 010 F3).
//!
//! One idempotent ensure that makes a repo drivable by the gated loop: the
//! five canonical `status/*` labels, a Projects v2 board linked **or created**
//! and bound (the F1 binding), the `.agentum-harness/` scaffold, and a
//! consent-gated commit+push of the scaffold CONTRACT files (D8). Plus the
//! template mode: `gh repo create --template` + clone for a brand-new repo.
//!
//! Domain logic lives here at crate root (the `linear.rs` /
//! `github_projects.rs` precedent); `routes/provision.rs` stays a thin wire
//! layer. Everything IO-adjacent is injectable — `program` (fake `gh`) and
//! `bindings_path` (temp file) — so the AC-10 run-twice test runs hermetically
//! with real git in temp repos and zero env mutation.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::github_projects::{
    self, BoardBinding, ResolvedMapping, StatusMapping, StatusNames, StatusOption,
};
use crate::task_sink::{GithubStateMap, TrackerPhase, gh_label_ensure_argv, github_status_color};

/// The fixed escalation label, duplicated from `task_sink::GITHUB_BLOCKED_LABEL`
/// (private there; the F3 boundary allows only the two builder-fn widenings —
/// the `github_projects::gh_bin` duplication precedent). Keep in sync with
/// `task_sink.rs`.
const BLOCKED_LABEL: (&str, &str) = ("status/blocked", "b60205");

/// Provisioning ops outlive API calls (template create+clone, push ride the
/// network), so the bound is wider than `run_gh`'s 30 s. A hung binary still
/// degrades to a reason string, never a stalled request.
const PROVISION_TIMEOUT_SECS: u64 = 120;

// ─── Pure argv builders (pinned by tests) ───────────────────────────────────

/// Pure argv: create a repo from a template and clone it into `./<name>`
/// (run with cwd = the parent directory — `gh` clones into `./<repo-name>`).
/// The template must be marked "Template repository" on GitHub; when it
/// isn't, `gh`'s stderr says so and the caller surfaces it verbatim.
fn gh_repo_create_from_template_argv<'a>(
    slug: &'a str,
    template: &'a str,
    private: bool,
) -> [&'a str; 7] {
    [
        "repo",
        "create",
        slug,
        "--template",
        template,
        if private { "--private" } else { "--public" },
        "--clone",
    ]
}

/// Pure argv: clone an existing repo into `./<repo-name>` (cwd = parent dir) —
/// the AC-10 "template-create skipped when the repo exists" arm.
fn gh_repo_clone_argv(slug: &str) -> [&str; 3] {
    ["repo", "clone", slug]
}

/// Pure argv: repo-existence probe — exits non-zero when the repo is missing.
fn gh_repo_view_argv(slug: &str) -> [&str; 5] {
    ["repo", "view", slug, "--json", "nameWithOwner"]
}

/// Pure argv: create a Projects v2 board. `--format json` prints the created
/// project as ONE JSON object; the shape is frozen (from a real gh 2.92.0) in
/// [`parse_project_create_output`]'s fixture.
fn gh_project_create_argv<'a>(owner: &'a str, title: &'a str) -> [&'a str; 8] {
    [
        "project", "create", "--owner", owner, "--title", title, "--format", "json",
    ]
}

/// First ~200 chars of an unexpected output, char-boundary safe, for error
/// messages that must quote what `gh` actually printed.
fn snippet(s: &str) -> String {
    let t = s.trim();
    if t.len() <= 200 {
        return t.to_string();
    }
    let mut end = 200;
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &t[..end])
}

/// Parse `gh project create --format json` stdout → the created project's
/// number — the ONE field the subsequent F1 discovery call needs (id/title are
/// re-fetched there). Defensive: garbage or a missing number classifies to an
/// `Err` quoting the unexpected output, never a panic.
fn parse_project_create_output(stdout: &str) -> Result<i64, String> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        format!(
            "gh project create returned unexpected output: {}",
            snippet(stdout)
        )
    })?;
    v.get("number")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            format!(
                "gh project create returned no project number: {}",
                snippet(stdout)
            )
        })
}

// ─── Process runner ─────────────────────────────────────────────────────────

/// Run one binary (gh or git) from an explicit cwd, RETURNING stdout. The
/// provision sibling of `github_projects::run_gh_capture` with a wider bound
/// and a caller-chosen cwd (template create/clone need cwd = the parent
/// directory; git needs cwd = the workdir). On failure the stderr is surfaced
/// (length-bounded) VERBATIM — e.g. gh's "not a template repository" must
/// reach the user unedited (handoff deviation-risk 3).
async fn run_in(program: &str, args: &[&str], cwd: &Path) -> Result<String, String> {
    let fut = crate::task_sink::output_with_etxtbsy_retry(program, args, cwd);
    let output =
        match tokio::time::timeout(std::time::Duration::from_secs(PROVISION_TIMEOUT_SECS), fut)
            .await
        {
            Err(_) => return Err(format!("{program} timed out")),
            Ok(Err(e)) => return Err(format!("failed to run `{program}`: {e}")),
            Ok(Ok(o)) => o,
        };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let mut msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if msg.is_empty() {
        msg = format!("{program} exited with {}", output.status);
    }
    if msg.len() > 400 {
        let mut end = 400;
        while !msg.is_char_boundary(end) {
            end -= 1;
        }
        msg.truncate(end);
        msg.push('…');
    }
    Err(msg)
}

// ─── Template mode ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct TemplateRepoResult {
    pub slug: String,
    pub path: PathBuf,
    pub created: bool,
}

/// Create-or-adopt a repo from a template (spec 010 §5.1 template mode):
/// 1. `directory/<name>/.git` exists ⇒ done, `created: false` (local
///    idempotency — never re-create or re-clone over a working tree);
/// 2. probe `gh repo view` — an existing remote repo is CLONED, not
///    re-created (AC 10); a probe miss falls through to
///    `gh repo create --template … --clone`. A probe that failed for
///    auth/network reasons surfaces through the create call's own stderr.
pub(crate) async fn create_repo_from_template(
    program: &str,
    owner: &str,
    name: &str,
    template: &str,
    directory: &Path,
    private: bool,
) -> Result<TemplateRepoResult, String> {
    let slug = format!("{owner}/{name}");
    let target = directory.join(name);
    if target.join(".git").exists() {
        return Ok(TemplateRepoResult {
            slug,
            path: target,
            created: false,
        });
    }
    let exists = run_in(program, &gh_repo_view_argv(&slug), directory)
        .await
        .is_ok();
    if exists {
        run_in(program, &gh_repo_clone_argv(&slug), directory).await?;
    } else {
        run_in(
            program,
            &gh_repo_create_from_template_argv(&slug, template, private),
            directory,
        )
        .await?;
    }
    // gh clones into ./<name>; a success that produced no clone is a lie we
    // must not propagate as a usable path.
    if !target.join(".git").exists() {
        return Err(format!(
            "gh succeeded but {} was not created",
            target.display()
        ));
    }
    Ok(TemplateRepoResult {
        slug,
        path: target,
        created: !exists,
    })
}

// ─── The one idempotent provisioning ensure ─────────────────────────────────

/// Which board the provision should bind — link an existing project or create
/// one first (D5). `None` on the ctx = no board requested (labels + scaffold
/// still ensure; an EXISTING binding still reports "already bound").
#[derive(Debug, Clone)]
pub(crate) enum ProjectChoice {
    Link {
        owner: String,
        owner_type: String,
        number: i64,
    },
    Create {
        owner: String,
        owner_type: String,
        title: String,
    },
}

/// Everything [`provision_repo`] needs, injectable for the run-twice test
/// (the finding-6 discipline extended to the bindings path and — mirroring
/// F2's seam-`map` injection — the label-name map, so tests never read the
/// user's real `github.json`).
pub(crate) struct ProvisionCtx<'a> {
    pub program: &'a str,
    /// `None` → the real `github_projects.json`; `Some` → a test temp file.
    pub bindings_path: Option<&'a Path>,
    pub workdir: &'a Path,
    pub slug: &'a str,
    pub project: Option<ProjectChoice>,
    /// Wizard-edited override; `None` = auto-resolve via the F1 fuzzy mapper.
    pub status_mapping: Option<StatusMapping>,
    pub done_closes_issue: bool,
    /// D8 consent — explicit on the wire, default ON only at the UI layer.
    pub commit_scaffold: bool,
    /// Route passes `GithubStateMap::from_env()`; tests pass `Default`.
    pub state_map: GithubStateMap,
}

/// One provisioning step's outcome. `ok: false` is a warning at every surface
/// (provision is best-effort per step); `changed: false` on a re-run is the
/// AC-10 idempotency signal.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StepReport {
    pub ok: bool,
    pub changed: bool,
    pub detail: String,
}

impl StepReport {
    fn ok(changed: bool, detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            changed,
            detail: detail.into(),
        }
    }
    fn failed(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            changed: false,
            detail: detail.into(),
        }
    }
}

/// The consent-gated commit step's outcome. A red push is `pushed: false` +
/// `error`, never a failed provision (D8: the workspace stays usable).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CommitReport {
    pub committed: bool,
    pub pushed: bool,
    /// The workdir's CURRENT branch (`git rev-parse --abbrev-ref HEAD`) —
    /// provisioning never checks out or switches branches. Empty when the
    /// commit step was skipped or the workdir is not a git repo.
    pub branch: String,
    pub error: Option<String>,
}

impl CommitReport {
    fn skipped() -> Self {
        Self {
            committed: false,
            pushed: false,
            branch: String::new(),
            error: None,
        }
    }
}

/// The whole provision run, per-step. Field names are single words, so the
/// derived JSON is already the camelCase wire shape.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProvisionReport {
    pub labels: StepReport,
    pub project: StepReport,
    pub binding: StepReport,
    pub scaffold: StepReport,
    pub commit: CommitReport,
}

/// The ONE idempotent provisioning ensure (spec 010 §5.1). Every step is
/// independent and best-effort — a red step is reported, never propagated;
/// only request-shape/missing-workdir errors are hard 4xx at the route layer.
pub(crate) async fn provision_repo(ctx: ProvisionCtx<'_>) -> ProvisionReport {
    let labels = ensure_labels(&ctx).await;
    let (project, binding) = ensure_project_binding(&ctx).await;
    let scaffold = ensure_scaffold(ctx.workdir).await;
    let commit = if ctx.commit_scaffold {
        commit_scaffold_files(ctx.workdir).await
    } else {
        // §6.8: a provision without the commit step also keeps the scaffold's
        // blanket `*` self-ignore untouched — nothing here rewrites it.
        CommitReport::skipped()
    };
    ProvisionReport {
        labels,
        project,
        binding,
        scaffold,
        commit,
    }
}

/// Step 1 — the five-label ensure: the four CONFIGURED pipeline names +
/// the fixed blocked label. Deliberately provision's OWN loop over the
/// `pub(crate)`-widened builders — never a refactor of
/// `github_transition_with`'s pinned ensure sequence (AC 8 forbids).
/// `--force` makes re-runs converge, so `changed` is structurally `false`
/// (gh does not report created-vs-updated; "no duplicate labels" holds by
/// gh contract).
async fn ensure_labels(ctx: &ProvisionCtx<'_>) -> StepReport {
    let neutral = crate::task_sink::neutral_cwd();
    let mut errors: Vec<String> = Vec::new();
    for phase in [
        TrackerPhase::Todo,
        TrackerPhase::InProgress,
        TrackerPhase::ReadyToTest,
        TrackerPhase::Done,
    ] {
        let name = ctx.state_map.label_for(phase);
        if let Err(e) = run_in(
            ctx.program,
            &gh_label_ensure_argv(name, ctx.slug, github_status_color(phase)),
            &neutral,
        )
        .await
        {
            errors.push(format!("{name}: {e}"));
        }
    }
    if let Err(e) = run_in(
        ctx.program,
        &gh_label_ensure_argv(BLOCKED_LABEL.0, ctx.slug, BLOCKED_LABEL.1),
        &neutral,
    )
    .await
    {
        errors.push(format!("{}: {e}", BLOCKED_LABEL.0));
    }
    if errors.is_empty() {
        StepReport::ok(false, "ensured the five status labels")
    } else {
        StepReport::failed(format!("label ensure failed: {}", errors.join("; ")))
    }
}

fn read_existing_binding(ctx: &ProvisionCtx<'_>) -> Option<BoardBinding> {
    match ctx.bindings_path {
        Some(p) => github_projects::binding_for_slug_at(p, ctx.slug),
        None => github_projects::binding_for_slug(ctx.slug),
    }
}

/// Step 2 — project link-or-create + bind, GUARDED by the binding: an
/// existing binding short-circuits create+bind ENTIRELY, whatever the request
/// says — THE idempotency rule the AC-10 run-twice test pins ("no second
/// project"). Returns `(project step, binding step)`.
async fn ensure_project_binding(ctx: &ProvisionCtx<'_>) -> (StepReport, StepReport) {
    if let Some(existing) = read_existing_binding(ctx) {
        let what = existing
            .project_title
            .filter(|t| !t.is_empty())
            .unwrap_or(existing.project_id);
        let detail = format!("already bound to {what}");
        return (
            StepReport::ok(false, detail.clone()),
            StepReport::ok(false, detail),
        );
    }
    let Some(choice) = &ctx.project else {
        let detail = "no project requested — bind a board later in Settings → Integrations";
        return (StepReport::ok(false, detail), StepReport::ok(false, detail));
    };

    let (owner, owner_type, number, project_step) = match choice {
        ProjectChoice::Link {
            owner,
            owner_type,
            number,
        } => (
            owner.clone(),
            owner_type.clone(),
            *number,
            StepReport::ok(false, format!("linked existing project #{number}")),
        ),
        ProjectChoice::Create {
            owner,
            owner_type,
            title,
        } => {
            let out = match run_in(
                ctx.program,
                &gh_project_create_argv(owner, title),
                &crate::task_sink::neutral_cwd(),
            )
            .await
            {
                Ok(out) => out,
                Err(e) => {
                    let detail = format!("project create failed: {e}");
                    return (
                        StepReport::failed(detail),
                        StepReport::failed("not bound (project create failed)"),
                    );
                }
            };
            let number = match parse_project_create_output(&out) {
                Ok(n) => n,
                Err(e) => {
                    return (
                        StepReport::failed(e),
                        StepReport::failed("not bound (could not read the created project)"),
                    );
                }
            };
            (
                owner.clone(),
                owner_type.clone(),
                number,
                StepReport::ok(true, format!("created project \"{title}\" (#{number})")),
            )
        }
    };

    // The F1 discovery — one gh call, doubling as the scope probe; its
    // classified message (e.g. the `gh auth refresh -s project` remedy)
    // rides the step detail verbatim.
    let discovered = match github_projects::discover_status_field(
        ctx.program,
        &owner,
        &owner_type,
        number,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            return (
                project_step,
                StepReport::failed(format!("not bound — discovery failed: {}", e.message)),
            );
        }
    };

    // Wizard override wins; otherwise the F1 fuzzy mapper (a created board
    // carries GitHub's default Todo / In Progress / Done — the two locked
    // fallbacks resolve it, D5-visible in the editor).
    let (mapping, names) = match &ctx.status_mapping {
        Some(m) => (m.clone(), names_for_ids(m, &discovered.options)),
        None => match github_projects::resolve_status_mapping(&discovered.options) {
            Ok(resolved) => (
                mapping_from_resolved(&resolved),
                Some(names_from_resolved(&resolved)),
            ),
            Err(refusal) => {
                return (
                    project_step,
                    StepReport::failed(format!(
                        "not bound — {refusal} (complete the mapping in Settings → Integrations)"
                    )),
                );
            }
        },
    };

    let title = if discovered.project_title.is_empty() {
        discovered.project_id.clone()
    } else {
        discovered.project_title.clone()
    };
    let new_binding = BoardBinding {
        project_id: discovered.project_id,
        status_field_id: discovered.status_field_id,
        status_mapping: mapping,
        done_closes_issue: ctx.done_closes_issue,
        project_title: Some(discovered.project_title).filter(|t| !t.is_empty()),
        project_owner: Some(owner),
        project_owner_type: Some(owner_type),
        project_number: Some(number),
        option_names: names,
    };
    let upsert = match ctx.bindings_path {
        Some(p) => github_projects::upsert_binding_at(p, ctx.slug, new_binding),
        None => github_projects::upsert_binding(ctx.slug, new_binding),
    };
    match upsert {
        Ok(()) => (
            project_step,
            StepReport::ok(true, format!("bound {} → {title}", ctx.slug)),
        ),
        Err(e) => (
            project_step,
            StepReport::failed(format!("not bound — could not persist the binding: {e}")),
        ),
    }
}

fn mapping_from_resolved(r: &ResolvedMapping) -> StatusMapping {
    StatusMapping {
        todo: r.todo.option_id.clone(),
        in_progress: r.in_progress.option_id.clone(),
        // #379: auto-provisioning now carries the In Review / PR option too
        // (a Review/PR column if the board has one, else the In Progress
        // fallback the resolver picked).
        in_review: r.in_review.option_id.clone(),
        ready_to_test: r.ready_to_test.option_id.clone(),
        done: r.done.option_id.clone(),
        blocked: r.blocked.option_id.clone(),
    }
}

fn names_from_resolved(r: &ResolvedMapping) -> StatusNames {
    StatusNames {
        todo: r.todo.option_name.clone(),
        in_progress: r.in_progress.option_name.clone(),
        in_review: r.in_review.option_name.clone(),
        ready_to_test: r.ready_to_test.option_name.clone(),
        done: r.done.option_name.clone(),
        blocked: r.blocked.option_name.clone(),
    }
}

/// Display metadata for an override mapping: look each option ID up in the
/// discovered options (unknown → empty name; names are never used at write
/// time, so a miss is cosmetic).
fn names_for_ids(m: &StatusMapping, options: &[StatusOption]) -> Option<StatusNames> {
    let name_of = |id: &str| -> String {
        options
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.name.clone())
            .unwrap_or_default()
    };
    Some(StatusNames {
        todo: name_of(&m.todo),
        in_progress: name_of(&m.in_progress),
        in_review: name_of(&m.in_review),
        ready_to_test: name_of(&m.ready_to_test),
        done: name_of(&m.done),
        blocked: name_of(&m.blocked),
    })
}

/// Step 3 — the scaffold, wrapping the UNTOUCHED `scaffold_harness`
/// (keep-existing = already idempotent); `changed` mirrors its written list.
async fn ensure_scaffold(workdir: &Path) -> StepReport {
    match crate::harness::scaffold_harness(workdir).await {
        Ok(s) if s.written.is_empty() => StepReport::ok(false, "scaffold already present"),
        Ok(s) => StepReport::ok(true, format!("wrote {}", s.written.join(", "))),
        Err(e) => StepReport::failed(format!("scaffold failed: {e}")),
    }
}

// ─── The consent-gated commit step (D8) ─────────────────────────────────────

/// The five CONTRACT paths the commit stages — the server twin of the UI's
/// pure `provisionCommitFileList()` (the D8 consent lists exactly these).
/// Engine-written state (`feature_list.json`, `handoff.md`, `qa/`) is
/// deliberately absent: it stays gitignored and is never staged.
const COMMIT_PATHS: [&str; 5] = [
    ".agentum-harness/.gitignore",
    ".agentum-harness/AGENTS.md",
    ".agentum-harness/init.sh",
    ".agentum-harness/verify.sh",
    ".agentum-harness/qa.sh",
];

/// The state-only replacement for the scaffold's blanket `*` self-ignore:
/// the contract files become committable while everything the ENGINE writes
/// (backlog state, handoffs, QA verdicts) stays out of `git status` — losing
/// that would re-import the exact worktree noise `harness/types.rs`'s
/// self-ignore exists to prevent (§6.8).
const STATE_ONLY_GITIGNORE: &str = "\
# agentum: engine-written runtime state stays untracked; the contract files
# (AGENTS.md, init.sh, verify.sh, qa.sh, this file) are committable.
feature_list.json
handoff.md
qa/
";

/// Rewrite `.agentum-harness/.gitignore` to the state-only ignore,
/// write-if-different. Returns whether it wrote. ONLY the consent-gated
/// commit path calls this — declining the commit keeps the blanket `*`.
fn rewrite_state_only_gitignore(workdir: &Path) -> Result<bool, String> {
    let path = workdir.join(".agentum-harness").join(".gitignore");
    if std::fs::read_to_string(&path).ok().as_deref() == Some(STATE_ONLY_GITIGNORE) {
        return Ok(false);
    }
    std::fs::write(&path, STATE_ONLY_GITIGNORE)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(true)
}

/// Step 4 — rewrite the self-ignore, stage the five contract paths, commit
/// (porcelain-empty ⇒ no commit — the AC-10 unchanged-commit-count rule) and
/// plain-push. Never `--force`; never a checkout/branch switch (the commit
/// lands on the workdir's CURRENT branch, reported); no AI-attribution
/// trailer (D8 + the standing repo-wide git rule). Every red git step folds
/// into the report — non-fatal by contract.
async fn commit_scaffold_files(workdir: &Path) -> CommitReport {
    fn fail(branch: &str, error: String) -> CommitReport {
        CommitReport {
            committed: false,
            pushed: false,
            branch: branch.to_string(),
            error: Some(error),
        }
    }
    let branch = run_in("git", &["rev-parse", "--abbrev-ref", "HEAD"], workdir)
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if let Err(e) = rewrite_state_only_gitignore(workdir) {
        return fail(&branch, e);
    }
    let mut add_args: Vec<&str> = vec!["add", "--"];
    add_args.extend(COMMIT_PATHS);
    if let Err(e) = run_in("git", &add_args, workdir).await {
        return fail(&branch, format!("git add failed: {e}"));
    }
    // Nothing staged and nothing dirty under the harness dir ⇒ the previous
    // provision commit already holds — no commit, count unchanged (AC 10).
    match run_in(
        "git",
        &["status", "--porcelain", "--", ".agentum-harness"],
        workdir,
    )
    .await
    {
        Ok(out) if out.trim().is_empty() => {
            return CommitReport {
                committed: false,
                pushed: false,
                branch,
                error: None,
            };
        }
        Ok(_) => {}
        Err(e) => return fail(&branch, format!("git status failed: {e}")),
    }
    if let Err(e) = run_in(
        "git",
        &["commit", "-m", "chore: provision agentum harness scaffold"],
        workdir,
    )
    .await
    {
        return fail(&branch, format!("git commit failed: {e}"));
    }
    // Plain push. A red push leaves the workspace usable — the commit exists
    // locally; the error is surfaced for a manual push (D8).
    match run_in("git", &["push", "origin", "HEAD"], workdir).await {
        Ok(_) => CommitReport {
            committed: true,
            pushed: true,
            branch,
            error: None,
        },
        Err(e) => CommitReport {
            committed: true,
            pushed: false,
            branch,
            error: Some(format!("push failed: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Why: only the #[cfg(unix)] tests below use json!; an unconditional
    // import is an unused-import error on Windows under -D warnings.
    #[cfg(unix)]
    use serde_json::json;

    // ─── Argv pins ──────────────────────────────────────────────────────────

    #[test]
    fn gh_repo_create_from_template_argv_shape() {
        assert_eq!(
            gh_repo_create_from_template_argv(
                "acme/widgets",
                "goempirical/empirical-sdd-ddd-starter",
                true,
            ),
            [
                "repo",
                "create",
                "acme/widgets",
                "--template",
                "goempirical/empirical-sdd-ddd-starter",
                "--private",
                "--clone"
            ]
        );
        assert_eq!(
            gh_repo_create_from_template_argv("a/b", "t/u", false)[5],
            "--public"
        );
    }

    #[test]
    fn gh_repo_clone_argv_shape() {
        assert_eq!(
            gh_repo_clone_argv("acme/widgets"),
            ["repo", "clone", "acme/widgets"]
        );
        // The existence probe that decides create-vs-clone.
        assert_eq!(
            gh_repo_view_argv("acme/widgets"),
            ["repo", "view", "acme/widgets", "--json", "nameWithOwner"]
        );
    }

    #[test]
    fn gh_project_create_argv_shape() {
        assert_eq!(
            gh_project_create_argv("acme", "My Board"),
            [
                "project", "create", "--owner", "acme", "--title", "My Board", "--format", "json"
            ]
        );
    }

    /// The `--format json` shape FROZEN from a real `gh` 2.92.0 project
    /// object (handoff deviation-risk 2): `number` is the field we need;
    /// everything else is tolerated. Garbage classifies, never panics.
    #[test]
    fn parse_project_create_output_frozen_fixture() {
        let fixture = r#"{"closed":false,"fields":{"totalCount":13},"id":"PVT_kwHOAf4oMc4Ba2Qp","items":{"totalCount":0},"number":3,"owner":{"login":"acme","type":"User"},"public":false,"readme":"","shortDescription":"","title":"Board","url":"https://github.com/users/acme/projects/3"}"#;
        assert_eq!(parse_project_create_output(fixture).unwrap(), 3);

        let err = parse_project_create_output("gh: not json at all").unwrap_err();
        assert!(err.contains("unexpected output"), "{err}");
        assert!(err.contains("not json"), "quotes what gh printed: {err}");

        let err = parse_project_create_output(r#"{"ok":true}"#).unwrap_err();
        assert!(err.contains("no project number"), "{err}");
    }

    // ─── Test plumbing: real git in temp repos + a logging fake gh ──────────

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// `git check-ignore` exit status IS the answer: 0 = ignored, 1 = not.
    fn is_ignored(dir: &Path, path: &str) -> bool {
        std::process::Command::new("git")
            .args(["check-ignore", "-q", path])
            .current_dir(dir)
            .status()
            .expect("git runs")
            .success()
    }

    /// A temp repo with one initial commit. Repo-local identity + gpgsign off
    /// so the provision commit works on any machine/CI regardless of global
    /// git config. Unix-gated like every test that calls it — dead code on
    /// Windows fails -D warnings.
    #[cfg(unix)]
    fn init_repo(root: &Path) -> PathBuf {
        let workdir = root.join("repo");
        std::fs::create_dir_all(&workdir).unwrap();
        git(&workdir, &["init", "--quiet"]);
        git(&workdir, &["config", "user.email", "t@example.com"]);
        git(&workdir, &["config", "user.name", "Test"]);
        git(&workdir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(workdir.join("README.md"), "hi\n").unwrap();
        git(&workdir, &["add", "README.md"]);
        git(&workdir, &["commit", "--quiet", "-m", "init"]);
        workdir
    }

    /// The run-twice fixture: the repo above + a bare `origin` so the plain
    /// push has somewhere real to land.
    #[cfg(unix)]
    fn init_repo_with_origin(root: &Path) -> PathBuf {
        let workdir = init_repo(root);
        git(root, &["init", "--quiet", "--bare", "origin.git"]);
        let origin = root.join("origin.git");
        git(
            &workdir,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        workdir
    }

    #[cfg(unix)]
    fn commit_count(workdir: &Path) -> u32 {
        git(workdir, &["rev-list", "--count", "HEAD"])
            .trim()
            .parse()
            .unwrap()
    }

    /// Fake `gh` for provisioning: logs every argv line; answers
    /// `project create` with the frozen created-project JSON and the
    /// discovery GraphQL with a default board (Todo / In Progress / Done —
    /// exactly what a fresh `gh project create` board carries).
    #[cfg(unix)]
    fn write_provision_fake_gh(dir: &Path, log: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let discovery = json!({"data": {"user": {"projectV2": {"id": "PVT_new", "title": "Board",
            "field": {"__typename": "ProjectV2SingleSelectField", "id": "PVTSSF_1",
            "options": [{"id": "o1", "name": "Todo"}, {"id": "o2", "name": "In Progress"},
                        {"id": "o3", "name": "Done"}]}}}}})
        .to_string();
        let created = json!({"closed": false, "id": "PVT_new", "number": 7,
            "owner": {"login": "acme", "type": "User"}, "public": false,
            "title": "Board", "url": "https://github.com/users/acme/projects/7"})
        .to_string();
        let script = dir.join("gh-fake");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> \"{log}\"\n\
                 case \"$1 $2\" in\n\
                 \x20 \"project create\") printf '%s\\n' '{created}' ;;\n\
                 \x20 \"api graphql\") printf '%s\\n' '{discovery}' ;;\n\
                 esac\n\
                 exit 0\n",
                log = log.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    fn sample_binding() -> BoardBinding {
        BoardBinding {
            project_id: "PVT_old".into(),
            status_field_id: "PVTSSF_old".into(),
            status_mapping: StatusMapping {
                todo: "t".into(),
                in_progress: "i".into(),
                in_review: String::new(),
                ready_to_test: "r".into(),
                done: "d".into(),
                blocked: "b".into(),
            },
            done_closes_issue: true,
            project_title: Some("Old Board".into()),
            project_owner: Some("acme".into()),
            project_owner_type: Some("user".into()),
            project_number: Some(1),
            option_names: None,
        }
    }

    // ─── The AC-10 pin (written FIRST, before the commit step) ──────────────

    /// AC 10: re-running provisioning against an already-provisioned repo
    /// changes NOTHING — no second project (the binding guard), binding file
    /// byte-identical, scaffold `changed: false`, commit count unchanged.
    #[cfg(unix)]
    #[tokio::test]
    async fn provision_run_twice_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let workdir = init_repo_with_origin(dir.path());
        let log = dir.path().join("gh.log");
        let gh = write_provision_fake_gh(dir.path(), &log);
        let bindings = dir.path().join("bindings.json");
        let ctx = || ProvisionCtx {
            program: gh.to_str().unwrap(),
            bindings_path: Some(&bindings),
            workdir: &workdir,
            slug: "acme/run-twice",
            project: Some(ProjectChoice::Create {
                owner: "acme".into(),
                owner_type: "user".into(),
                title: "Board".into(),
            }),
            status_mapping: None,
            done_closes_issue: true,
            commit_scaffold: true,
            state_map: GithubStateMap::default(),
        };

        // Run 1: labels + create+bind + scaffold + commit (+1) + push.
        let r1 = provision_repo(ctx()).await;
        assert!(r1.labels.ok, "{:?}", r1.labels);
        assert!(r1.project.ok && r1.project.changed, "{:?}", r1.project);
        assert!(r1.binding.ok && r1.binding.changed, "{:?}", r1.binding);
        assert!(r1.scaffold.ok && r1.scaffold.changed, "{:?}", r1.scaffold);
        assert!(r1.commit.committed && r1.commit.pushed, "{:?}", r1.commit);
        assert!(!r1.commit.branch.is_empty(), "reports the branch");
        let count1 = commit_count(&workdir);
        assert_eq!(count1, 2, "initial + the one provision commit");
        let calls1 = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            calls1.matches("project create").count(),
            1,
            "one project created: {calls1}"
        );
        assert_eq!(
            calls1
                .lines()
                .filter(|l| l.starts_with("label create"))
                .count(),
            5,
            "five label ensures: {calls1}"
        );
        assert!(
            calls1.contains("label create status/blocked"),
            "the fixed blocked label rides its own ensure: {calls1}"
        );
        let run1_lines = calls1.lines().count();
        let binding1 = std::fs::read_to_string(&bindings).unwrap();
        assert!(binding1.contains("PVT_new"), "bound to the created project");

        // Run 2: NO `project create`, NO discovery (bind skipped entirely),
        // binding file unchanged, scaffold unchanged, commit count equal.
        let r2 = provision_repo(ctx()).await;
        assert!(
            r2.project.ok && !r2.project.changed,
            "already bound: {:?}",
            r2.project
        );
        assert!(
            r2.project.detail.contains("already bound"),
            "{:?}",
            r2.project
        );
        assert!(!r2.binding.changed, "{:?}", r2.binding);
        assert!(!r2.scaffold.changed, "{:?}", r2.scaffold);
        assert!(!r2.commit.committed, "{:?}", r2.commit);
        assert!(r2.commit.error.is_none(), "{:?}", r2.commit);
        let calls2 = std::fs::read_to_string(&log).unwrap();
        let run2: Vec<&str> = calls2.lines().skip(run1_lines).collect();
        assert!(
            run2.iter().all(|l| !l.contains("project create")),
            "no second project: {run2:?}"
        );
        assert!(
            run2.iter().all(|l| !l.contains("api graphql")),
            "create+bind skipped entirely: {run2:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&bindings).unwrap(),
            binding1,
            "binding file byte-identical"
        );
        assert_eq!(
            commit_count(&workdir),
            count1,
            "commit count unchanged (AC 10)"
        );
    }

    /// D8: consent OFF ⇒ no commit, no push, AND the scaffold's blanket `*`
    /// self-ignore stays untouched (§6.8).
    #[cfg(unix)]
    #[tokio::test]
    async fn provision_skips_commit_when_consent_off() {
        let dir = tempfile::tempdir().unwrap();
        let workdir = init_repo_with_origin(dir.path());
        let log = dir.path().join("gh.log");
        let gh = write_provision_fake_gh(dir.path(), &log);
        let bindings = dir.path().join("bindings.json");
        let report = provision_repo(ProvisionCtx {
            program: gh.to_str().unwrap(),
            bindings_path: Some(&bindings),
            workdir: &workdir,
            slug: "acme/consent-off",
            project: None,
            status_mapping: None,
            done_closes_issue: true,
            commit_scaffold: false,
            state_map: GithubStateMap::default(),
        })
        .await;
        assert!(report.scaffold.ok && report.scaffold.changed);
        assert!(!report.commit.committed && !report.commit.pushed);
        assert!(report.commit.error.is_none());
        assert_eq!(commit_count(&workdir), 1, "no provision commit");
        assert_eq!(
            std::fs::read_to_string(workdir.join(".agentum-harness/.gitignore")).unwrap(),
            "*\n",
            "blanket self-ignore untouched without consent (§6.8)"
        );
        // No board requested and none bound: both steps say so, ok.
        assert!(report.project.ok && !report.project.changed);
        assert!(report.project.detail.contains("no project requested"));
    }

    /// D8: a red push (here: no `origin` remote at all) is surfaced as
    /// `pushed: false` + error — the commit still lands locally and the
    /// report stays whole (non-fatal).
    #[cfg(unix)]
    #[tokio::test]
    async fn provision_red_push_is_nonfatal_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let workdir = init_repo(dir.path()); // deliberately NO origin
        let log = dir.path().join("gh.log");
        let gh = write_provision_fake_gh(dir.path(), &log);
        let bindings = dir.path().join("bindings.json");
        let report = provision_repo(ProvisionCtx {
            program: gh.to_str().unwrap(),
            bindings_path: Some(&bindings),
            workdir: &workdir,
            slug: "acme/red-push",
            project: None,
            status_mapping: None,
            done_closes_issue: true,
            commit_scaffold: true,
            state_map: GithubStateMap::default(),
        })
        .await;
        assert!(report.commit.committed, "{:?}", report.commit);
        assert!(!report.commit.pushed, "{:?}", report.commit);
        let err = report.commit.error.clone().expect("push error surfaced");
        assert!(!err.trim().is_empty());
        assert!(report.scaffold.ok, "the rest of the report stays whole");
        assert_eq!(commit_count(&workdir), 2, "the local commit still landed");
    }

    /// The AC-10 guard in isolation: an existing binding means the request's
    /// Create is IGNORED — no `gh project create`, no discovery, binding file
    /// untouched.
    #[cfg(unix)]
    #[tokio::test]
    async fn provision_with_existing_binding_never_creates_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let workdir = dir.path().join("plain");
        std::fs::create_dir_all(&workdir).unwrap();
        let log = dir.path().join("gh.log");
        let gh = write_provision_fake_gh(dir.path(), &log);
        let bindings = dir.path().join("bindings.json");
        github_projects::upsert_binding_at(&bindings, "acme/bound", sample_binding()).unwrap();
        let before = std::fs::read_to_string(&bindings).unwrap();

        let report = provision_repo(ProvisionCtx {
            program: gh.to_str().unwrap(),
            bindings_path: Some(&bindings),
            workdir: &workdir,
            slug: "acme/bound",
            project: Some(ProjectChoice::Create {
                owner: "acme".into(),
                owner_type: "user".into(),
                title: "Second Board".into(),
            }),
            status_mapping: None,
            done_closes_issue: true,
            commit_scaffold: false,
            state_map: GithubStateMap::default(),
        })
        .await;
        assert!(report.project.ok && !report.project.changed);
        assert!(
            report.project.detail.contains("already bound to Old Board"),
            "{:?}",
            report.project
        );
        assert!(!report.binding.changed);
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(!calls.contains("project create"), "{calls}");
        assert!(!calls.contains("api graphql"), "{calls}");
        assert_eq!(
            std::fs::read_to_string(&bindings).unwrap(),
            before,
            "binding untouched"
        );
    }

    /// §6.8: the rewrite is write-if-different, drops the blanket `*`, and —
    /// proven with REAL git — keeps every engine-written state path ignored
    /// while the contract files become trackable.
    #[tokio::test]
    async fn gitignore_rewrite_is_write_if_different_and_keeps_state_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let workdir = dir.path().join("repo");
        std::fs::create_dir_all(&workdir).unwrap();
        git(&workdir, &["init", "--quiet"]);
        crate::harness::scaffold_harness(&workdir).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workdir.join(".agentum-harness/.gitignore")).unwrap(),
            "*\n"
        );

        assert!(
            rewrite_state_only_gitignore(&workdir).unwrap(),
            "first rewrite writes"
        );
        let content = std::fs::read_to_string(workdir.join(".agentum-harness/.gitignore")).unwrap();
        for entry in ["feature_list.json", "handoff.md", "qa/"] {
            assert!(
                content.lines().any(|l| l == entry),
                "{entry} stays ignored: {content}"
            );
        }
        assert!(
            !content.lines().any(|l| l.trim() == "*"),
            "blanket ignore gone: {content}"
        );
        assert!(
            !rewrite_state_only_gitignore(&workdir).unwrap(),
            "second rewrite is a no-op (write-if-different)"
        );

        // Real-git semantics: engine state ignored, contract files trackable.
        for ignored in [
            ".agentum-harness/feature_list.json",
            ".agentum-harness/handoff.md",
            ".agentum-harness/qa/verdict.json",
        ] {
            assert!(is_ignored(&workdir, ignored), "{ignored} must stay ignored");
        }
        for tracked in COMMIT_PATHS {
            assert!(
                !is_ignored(&workdir, tracked),
                "{tracked} must be committable"
            );
        }
    }
}
