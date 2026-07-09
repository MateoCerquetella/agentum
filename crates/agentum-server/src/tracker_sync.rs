//! Session-lifecycle → tracker status sync (spec 012, F2–F4).
//!
//! Two thin layers that drive an item's status through the workspace's session
//! lifecycle by *calling* the one existing write seam
//! [`crate::task_sink::apply_tracker_transition`] (which spec 010 already taught
//! to move the Projects Status column for a bound repo) — this module writes no
//! label/Projects/Linear code of its own (invariant #1).
//!
//! - **Session-start reactor** (F2): a bus subscriber that, on `session.started`
//!   in a bound worktree, fires `InProgress`. Never inline in the launch path
//!   (invariant #2) — it hangs off the lifecycle bus the watchdog already emits.
//! - **PR/merge poller** (F3/F4): a bounded, backed-off `gh` loop that drives
//!   `InReview` on the first non-draft PR and `Done` on merge. No webhooks
//!   (invariant #6) → poll only.
//!
//! Every transition is idempotent, best-effort, and never-halt (invariant #3):
//! a failed transition logs and the session/poll proceeds. Advancement is guarded
//! by the pure monotonic [`next_phase_write`] (invariant #4) so status never
//! regresses (a reopened Done workspace does not drag the card back), the
//! session-start `InProgress` converges with the harness's own `InProgress`, and
//! the poller's `Done` is a restart-safe terminal (the persisted `tracker_phase`
//! excludes a merged workspace from the next tick).

use std::sync::Arc;

use agentum_core::Event;
use agentum_store::Store;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::task_sink::{TrackerEmit, TrackerPhase, apply_tracker_transition, parse_tracker_phase};

/// The canonical monotonic rank of a pipeline phase (spec 012 §4):
///
/// ```text
/// Todo(0) < InProgress(1) < InReview(2) < ReadyToTest(3) < Done(4)
/// ```
///
/// The `InReview(2)` slot is reserved for F3 so adding that variant never shifts
/// the others. Chosen so `InReview`'s nearest-earlier mapped phase is
/// `InProgress` (the spec's Projects fallback) and a gated run's `ReadyToTest`
/// (unit-green) can never regress to `InReview` when a PR opens. `Done(4)` always
/// wins — merge is terminal.
fn phase_rank(phase: TrackerPhase) -> i8 {
    match phase {
        TrackerPhase::Todo => 0,
        TrackerPhase::InProgress => 1,
        TrackerPhase::InReview => 2,
        TrackerPhase::ReadyToTest => 3,
        TrackerPhase::Done => 4,
    }
}

/// The lowercase wire form of a phase — the value persisted as a worktree's
/// `tracker_phase` and re-parsed by [`parse_tracker_phase`] on the next event.
/// The exact inverse of `parse_tracker_phase` (round-trips). A thin delegate
/// since spec 014 moved the table onto the seam type ([`TrackerPhase::wire_str`])
/// so the emitted event payload and the persisted value can never drift.
pub(crate) fn tracker_phase_wire(phase: TrackerPhase) -> &'static str {
    phase.wire_str()
}

/// The monotonic-forward guard (invariant #4). Returns `Some(target)` only when
/// `target` ranks strictly above the worktree's persisted phase; `None` when the
/// item is already at or past `target` (idempotent / no-thrash / no regress).
///
/// An absent or unparseable `current` ranks below `Todo`, so a first transition
/// always advances. Because the guard reads the *persisted* phase, a session
/// re-start, reconnect, or extra tab is a no-op, the session-start `InProgress`
/// converges with the harness's own `InProgress`, and a merged workspace's `Done`
/// is a restart-safe terminal.
pub(crate) fn next_phase_write(
    current: Option<&str>,
    target: TrackerPhase,
) -> Option<TrackerPhase> {
    let current_rank = current
        .and_then(parse_tracker_phase)
        .map(phase_rank)
        .unwrap_or(-1);
    if current_rank < phase_rank(target) {
        Some(target)
    } else {
        None
    }
}

/// Resolve a worktree's persisted bind coords into `(provider, tracker_url)`, or
/// `None` for an unbound worktree (invariant #5, fail-closed). A provider outside
/// the supported set or an empty URL yields no binding — never a fabricated one.
pub(crate) fn resolve_binding(
    provider: Option<&str>,
    url: Option<&str>,
) -> Option<(String, String)> {
    let provider = provider.map(str::trim).filter(|p| !p.is_empty())?;
    if !matches!(provider, "github" | "linear") {
        return None;
    }
    let url = url.map(str::trim).filter(|u| !u.is_empty())?;
    Some((provider.to_string(), url.to_string()))
}

/// The session-start reactor's pure decision (AC 5–7): given a worktree's bind
/// coords + its persisted phase, what transition (if any) should a session start
/// fire? `None` for an unbound worktree (silent no-op) or one already ≥
/// `InProgress` (converges with the harness, blocks a Done→InProgress regress).
pub(crate) fn session_start_decision(
    provider: Option<&str>,
    url: Option<&str>,
    current_phase: Option<&str>,
) -> Option<(String, String, TrackerPhase)> {
    let (provider, url) = resolve_binding(provider, url)?;
    let target = next_phase_write(current_phase, TrackerPhase::InProgress)?;
    Some((provider, url, target))
}

/// The tracker id `apply_tracker_transition` needs per provider. The GitHub arm
/// ignores it (it parses `owner/repo` + number from the URL), so the URL doubles
/// as an inert id; the Linear arm uses the item identifier, which the worktree
/// persists as `linked_linear_issue` (falling back to the URL string).
fn tracker_id_for(provider: &str, url: &str, linked_linear_issue: Option<&str>) -> String {
    match provider {
        "linear" => linked_linear_issue
            .map(str::to_string)
            .unwrap_or_else(|| url.to_string()),
        _ => url.to_string(),
    }
}

/// React to one `session.started` event: map the session to its worktree by
/// workdir, and — if bound and not already advanced — fire `InProgress` and
/// persist the phase. Best-effort/never-halt (invariant #3): every miss is a
/// quiet return, every transport failure logs and is dropped.
async fn react_to_session_start(store: &Store, bus: &broadcast::Sender<Event>, session_id: Uuid) {
    let Ok(Some(session)) = store.get_session_by_id(session_id).await else {
        return;
    };
    let workdir = session.workdir;
    let Some(worktree) = crate::routes::worktrees::find_tracker_worktree_by_path(&workdir) else {
        return; // a plain, non-registered workdir — silent no-op (AC 7)
    };
    let Some((provider, url, target)) = session_start_decision(
        worktree.tracker_provider.as_deref(),
        worktree.tracker_url.as_deref(),
        worktree.tracker_phase.as_deref(),
    ) else {
        return; // unbound, or already ≥ InProgress (converges / no regress)
    };
    let tracker_id = tracker_id_for(&provider, &url, worktree.linked_linear_issue.as_deref());
    let emit = TrackerEmit {
        bus,
        worktree_id: Some(&worktree.id),
    };
    match apply_tracker_transition(store, &provider, &tracker_id, Some(&url), target, emit).await {
        Ok(result) => {
            tracing::info!(
                workdir = %workdir,
                provider = %provider,
                ?target,
                ?result,
                "session-start tracker transition"
            );
            // Persist the phase so the guard dedupes re-starts and the poller's
            // terminal-stop survives a reboot. A registry miss is a no-op.
            if let Err(e) = crate::routes::worktrees::persist_tracker_progress(
                &worktree.id,
                Some(tracker_phase_wire(target)),
                None,
            ) {
                tracing::warn!(error = %e, "persisting tracker_phase failed (non-fatal)");
            }
        }
        Err(e) => {
            tracing::warn!(workdir = %workdir, error = %e, "session-start tracker transition failed (non-fatal)");
        }
    }
}

/// The session-start reactor loop (F2): subscribe to the lifecycle bus and, on
/// each `session.started`, drive `InProgress` for a bound worktree. Runs forever
/// (until the bus closes at shutdown); a lagged receiver is skipped, never fatal.
/// Spawned at server boot alongside the other background workers.
pub async fn run_session_start_reactor(store: Arc<Store>, bus: broadcast::Sender<Event>) {
    let mut rx = bus.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                if event.kind == "session.started" {
                    if let Some(session_id) = event.session_id {
                        react_to_session_start(&store, &bus, session_id).await;
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

// ─── F3/F4: the PR-open/merge poller ────────────────────────────────────────
//
// No inbound webhooks on a self-hosted daemon (invariant #6) → a bounded,
// backed-off `gh` loop is the only sanctioned PR/merge detector. GitHub-only in
// v1. Bounded: a per-call timeout, a per-tick cap, and loop backoff on a
// wholly-failed tick keep it rate-limit friendly and never-halt (invariant #3).

/// Default poll cadence; override with `AGENTUM_TRACKER_POLL_SECS`.
const DEFAULT_POLL_SECS: u64 = 45;
/// Per-tick worktree cap so a large registry never fans out an unbounded burst.
const MAX_WORKTREES_PER_TICK: usize = 50;
/// Per-`gh`-call timeout so a hung request degrades to a skip, never a stall.
/// 30s matches the other `gh` runners — generous enough that a delayed spawn
/// under a saturated test run never trips it.
const GH_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The one PR a bound branch cares about, parsed from `gh pr list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrInfo {
    pub number: i64,
    pub is_draft: bool,
    pub state: String,
    pub url: String,
}

/// Parse `gh pr list --json number,state,isDraft,url` (a JSON array) into the
/// first PR, or `None` when the array is empty/malformed (no PR yet). Pure.
pub(crate) fn parse_pr_list(stdout: &str) -> Option<PrInfo> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let first = value.as_array()?.first()?;
    Some(PrInfo {
        number: first.get("number")?.as_i64()?,
        is_draft: first
            .get("isDraft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        state: first
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        url: first
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// The PR-open decision (F3): the first NON-draft PR advances a bound worktree
/// to InReview (guarded monotonic). A draft PR is not a trigger (spec open
/// question 5). Pure.
pub(crate) fn poll_pr_open_decision(
    current_phase: Option<&str>,
    pr: &PrInfo,
) -> Option<TrackerPhase> {
    if pr.is_draft {
        return None;
    }
    next_phase_write(current_phase, TrackerPhase::InReview)
}

/// Pure argv for the branch's PR probe (open PRs — the default `gh pr list`
/// state; a pre-poll merge is the explicitly out-of-scope edge, spec §non-goals).
fn pr_list_argv<'a>(slug: &'a str, branch: &'a str) -> [&'a str; 8] {
    [
        "pr",
        "list",
        "--head",
        branch,
        "--repo",
        slug,
        "--json",
        "number,state,isDraft,url",
    ]
}

/// One bounded `gh` call capturing stdout (the poller's runner; the binary comes
/// from the shared `github_projects::gh_bin` seam so tests inject a fake — no
/// fourth `gh_bin` dup, invariant #8). `Err` on timeout / spawn failure /
/// non-zero exit — the caller logs and skips, never halts.
async fn run_gh_capture(program: &str, args: &[&str]) -> Result<String, String> {
    // Pin the cwd to the always-present neutral dir ($HOME), like every other
    // `gh` runner: the `--repo` calls don't need the repo cwd, and inheriting a
    // deleted test/working dir makes process spawn itself fail (ENOENT on getcwd).
    let fut = tokio::process::Command::new(program)
        .args(args)
        .current_dir(crate::task_sink::neutral_cwd())
        .output();
    let output = match tokio::time::timeout(GH_CALL_TIMEOUT, fut).await {
        Err(_) => return Err("gh timed out".into()),
        Ok(Err(e)) => return Err(format!("failed to run `{program}`: {e}")),
        Ok(Ok(o)) => o,
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if msg.is_empty() {
        format!("gh exited with {}", output.status)
    } else {
        msg
    })
}

/// `gh pr list` for a branch → the first PR (if any). `Ok(None)` = no PR yet;
/// `Err` = a `gh` failure the caller logs and skips (never-halt).
async fn pr_list_via_gh(program: &str, slug: &str, branch: &str) -> Result<Option<PrInfo>, String> {
    let out = run_gh_capture(program, &pr_list_argv(slug, branch)).await?;
    Ok(parse_pr_list(&out))
}

/// A known PR's merge state, from `gh pr view <n> --json state,mergedAt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrView {
    pub state: String,
    pub merged: bool,
}

/// Parse `gh pr view --json state,mergedAt` → `(state, merged)`. Merged iff the
/// state is `MERGED` or `mergedAt` is a non-null timestamp (either signal is
/// authoritative). Pure; `None` on malformed JSON.
pub(crate) fn parse_pr_view(stdout: &str) -> Option<PrView> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let state = value
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let merged = state.eq_ignore_ascii_case("MERGED")
        || value.get("mergedAt").map(|m| !m.is_null()).unwrap_or(false);
    Some(PrView { state, merged })
}

/// The merge decision (F4): a MERGED PR advances a bound worktree to Done
/// (terminal). Guarded monotonic — a worktree already at Done yields `None` (the
/// terminal-stop, so a re-observed merge never re-transitions / re-closes the
/// issue). Pure.
pub(crate) fn poll_pr_merge_decision(
    current_phase: Option<&str>,
    view: &PrView,
) -> Option<TrackerPhase> {
    if !view.merged {
        return None;
    }
    next_phase_write(current_phase, TrackerPhase::Done)
}

/// Pure argv for a known PR's merge probe.
fn pr_view_argv<'a>(number: &'a str, slug: &'a str) -> [&'a str; 7] {
    [
        "pr",
        "view",
        number,
        "--repo",
        slug,
        "--json",
        "state,mergedAt",
    ]
}

/// `gh pr view <n>` → the PR's merge state. `Err` = a `gh` failure the caller
/// logs and skips (never-halt).
async fn pr_view_via_gh(program: &str, slug: &str, number: i64) -> Result<PrView, String> {
    let number = number.to_string();
    let out = run_gh_capture(program, &pr_view_argv(&number, slug)).await?;
    parse_pr_view(&out).ok_or_else(|| format!("could not parse `gh pr view` output: {out}"))
}

/// One transition + persist for a poller-detected phase (github). Best-effort:
/// a transport failure logs and is dropped; the phase is persisted on success so
/// the guard/terminal-stop survives a reboot.
async fn drive_and_persist(
    store: &Store,
    bus: &broadcast::Sender<Event>,
    worktree_id: &str,
    tracker_url: &str,
    linked_linear_issue: Option<&str>,
    target: TrackerPhase,
) {
    let tracker_id = tracker_id_for("github", tracker_url, linked_linear_issue);
    let emit = TrackerEmit {
        bus,
        worktree_id: Some(worktree_id),
    };
    match apply_tracker_transition(
        store,
        "github",
        &tracker_id,
        Some(tracker_url),
        target,
        emit,
    )
    .await
    {
        Ok(result) => {
            tracing::info!(worktree = %worktree_id, ?target, ?result, "poller tracker transition");
            if let Err(e) = crate::routes::worktrees::persist_tracker_progress(
                worktree_id,
                Some(tracker_phase_wire(target)),
                None,
            ) {
                tracing::warn!(error = %e, "persisting tracker_phase failed (non-fatal)");
            }
        }
        Err(e) => {
            tracing::warn!(worktree = %worktree_id, error = %e, "poller tracker transition failed (non-fatal)");
        }
    }
}

/// One tick's `gh`-call accounting, so the loop can back off a wholly-failed tick.
#[derive(Debug, Default)]
struct PollTick {
    attempted: usize,
    failed: usize,
}

impl PollTick {
    /// Every `gh` call this tick failed (and there was at least one) — back off.
    fn all_failed(&self) -> bool {
        self.attempted > 0 && self.failed == self.attempted
    }
}

/// One poll tick: for each bound, non-Done github worktree with a branch, detect
/// its PR lifecycle. F3 handles PR-open → InReview + persist `linked_pr`. (F4
/// extends this with merge → Done for a worktree that already has a `linked_pr`.)
async fn poll_pr_lifecycle_once(
    store: &Store,
    bus: &broadcast::Sender<Event>,
    program: &str,
) -> PollTick {
    let mut tick = PollTick::default();
    let worktrees: Vec<crate::routes::worktrees::TrackerWorktree> =
        crate::routes::worktrees::list_tracker_worktrees()
            .into_iter()
            .filter(|w| {
                w.tracker_provider.as_deref() == Some("github")
                    && w.branch.is_some()
                    // Terminal-stop (F4 AC12): a merged workspace is excluded,
                    // restart-safe via the persisted phase.
                    && w.tracker_phase.as_deref() != Some("done")
            })
            .take(MAX_WORKTREES_PER_TICK)
            .collect();

    for w in worktrees {
        let Some(url) = w.tracker_url.as_deref() else {
            continue;
        };
        // The label/Projects target is the ISSUE (parsed from tracker_url); the
        // PR is queried by (repo, head-branch).
        let Some((slug, _number)) = crate::task_sink::github_slug_and_number_from_issue_url(url)
        else {
            continue;
        };
        let Some(branch) = w.branch.as_deref() else {
            continue;
        };

        if w.linked_pr.is_none() {
            // F3: no PR seen yet — probe for the first non-draft PR → InReview.
            tick.attempted += 1;
            match pr_list_via_gh(program, &slug, branch).await {
                Ok(Some(pr)) if !pr.is_draft => {
                    // Persist the PR number first so a later transition failure
                    // still records that a PR exists (avoids re-probing forever).
                    let _ = crate::routes::worktrees::persist_tracker_progress(
                        &w.id,
                        None,
                        Some(pr.number),
                    );
                    if poll_pr_open_decision(w.tracker_phase.as_deref(), &pr).is_some() {
                        drive_and_persist(
                            store,
                            bus,
                            &w.id,
                            url,
                            w.linked_linear_issue.as_deref(),
                            TrackerPhase::InReview,
                        )
                        .await;
                    }
                }
                Ok(_) => {} // no PR, or a draft PR — not a trigger yet
                Err(reason) => {
                    tick.failed += 1;
                    tracing::warn!(slug = %slug, branch = %branch, %reason, "gh pr list failed (non-fatal)");
                }
            }
        } else if let Some(pr_number) = w.linked_pr {
            // F4: a PR was seen — check for merge → Done (terminal). The
            // `tracker_phase != "done"` filter already excludes a done workspace,
            // so this only runs for an InReview one awaiting its merge.
            tick.attempted += 1;
            match pr_view_via_gh(program, &slug, pr_number).await {
                Ok(view) => {
                    if poll_pr_merge_decision(w.tracker_phase.as_deref(), &view).is_some() {
                        // Done moves the label + Project option to done and, per
                        // 010's `done_closes_issue`, closes the issue. Persisting
                        // "done" makes the terminal-stop restart-safe.
                        drive_and_persist(
                            store,
                            bus,
                            &w.id,
                            url,
                            w.linked_linear_issue.as_deref(),
                            TrackerPhase::Done,
                        )
                        .await;
                    }
                }
                Err(reason) => {
                    tick.failed += 1;
                    tracing::warn!(slug = %slug, pr = pr_number, %reason, "gh pr view failed (non-fatal)");
                }
            }
        }
    }
    tick
}

/// The PR/merge poller loop (F3/F4): every `AGENTUM_TRACKER_POLL_SECS` (default
/// 45s), drive InReview on the first non-draft PR and Done on merge, for every
/// bound github worktree. Bounded + backed-off + never-halt. Spawned at server
/// boot beside the other background workers.
pub async fn run_pr_merge_poller(store: Arc<Store>, bus: broadcast::Sender<Event>) {
    let secs = std::env::var("AGENTUM_TRACKER_POLL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_POLL_SECS);
    let base = std::time::Duration::from_secs(secs);
    let mut consecutive_failed_ticks: u32 = 0;
    loop {
        // Exponential backoff (capped) after a wholly-failed tick — rate-limit
        // friendly; a healthy tick resets to the base cadence.
        let wait = base * 2u32.pow(consecutive_failed_ticks.min(4));
        tokio::time::sleep(wait).await;
        let program = crate::github_projects::gh_bin();
        let tick = poll_pr_lifecycle_once(&store, &bus, &program).await;
        if tick.all_failed() {
            consecutive_failed_ticks = consecutive_failed_ticks.saturating_add(1);
        } else {
            consecutive_failed_ticks = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_phase_write_is_monotonic_and_idempotent() {
        // A first transition always advances (absent → below Todo).
        assert_eq!(
            next_phase_write(None, TrackerPhase::InProgress),
            Some(TrackerPhase::InProgress)
        );
        assert_eq!(
            next_phase_write(Some("todo"), TrackerPhase::InProgress),
            Some(TrackerPhase::InProgress)
        );
        // Idempotent: re-firing the same phase is a no-op (converges with harness).
        assert_eq!(
            next_phase_write(Some("in_progress"), TrackerPhase::InProgress),
            None
        );
        // No regress: Done never drags back to InProgress on a session reopen.
        assert_eq!(
            next_phase_write(Some("done"), TrackerPhase::InProgress),
            None
        );
        // Done always advances from any earlier phase, and is terminal.
        assert_eq!(
            next_phase_write(Some("in_progress"), TrackerPhase::Done),
            Some(TrackerPhase::Done)
        );
        assert_eq!(next_phase_write(Some("done"), TrackerPhase::Done), None);
        // An unparseable persisted phase ranks below everything → advances.
        assert_eq!(
            next_phase_write(Some("garbage"), TrackerPhase::InProgress),
            Some(TrackerPhase::InProgress)
        );
    }

    #[test]
    fn tracker_phase_wire_round_trips_through_parse() {
        for phase in [
            TrackerPhase::Todo,
            TrackerPhase::InProgress,
            TrackerPhase::ReadyToTest,
            TrackerPhase::Done,
        ] {
            assert_eq!(parse_tracker_phase(tracker_phase_wire(phase)), Some(phase));
        }
    }

    #[test]
    fn resolve_binding_is_fail_closed() {
        // A fully-formed github/linear bind resolves.
        assert_eq!(
            resolve_binding(Some("github"), Some("https://github.com/o/r/issues/1")),
            Some((
                "github".to_string(),
                "https://github.com/o/r/issues/1".to_string()
            ))
        );
        assert_eq!(
            resolve_binding(Some("linear"), Some("ENG-9")),
            Some(("linear".to_string(), "ENG-9".to_string()))
        );
        // Fail-closed: no provider, an unsupported provider, or an empty URL binds nothing.
        assert_eq!(resolve_binding(None, Some("https://x")), None);
        assert_eq!(resolve_binding(Some("gitlab"), Some("https://x")), None);
        assert_eq!(resolve_binding(Some("github"), None), None);
        assert_eq!(resolve_binding(Some("github"), Some("   ")), None);
        assert_eq!(resolve_binding(Some(""), Some("https://x")), None);
    }

    #[test]
    fn session_start_fires_inprogress_for_bound_worktree() {
        let decision = session_start_decision(
            Some("github"),
            Some("https://github.com/o/r/issues/1"),
            None,
        );
        assert_eq!(
            decision,
            Some((
                "github".to_string(),
                "https://github.com/o/r/issues/1".to_string(),
                TrackerPhase::InProgress
            ))
        );
    }

    #[test]
    fn session_start_is_no_op_for_unbound_worktree() {
        // No provider/url at all → nothing to drive.
        assert_eq!(session_start_decision(None, None, None), None);
        // A provider but no URL (partial bind) → fail-closed, no transition.
        assert_eq!(session_start_decision(Some("github"), None, None), None);
    }

    #[test]
    fn session_start_converges_with_harness_inprogress_no_thrash() {
        // Already InProgress (e.g. the harness fired it) → no duplicate.
        assert_eq!(
            session_start_decision(
                Some("github"),
                Some("https://github.com/o/r/issues/1"),
                Some("in_progress")
            ),
            None
        );
        // Already Done → a re-opened session never regresses the card.
        assert_eq!(
            session_start_decision(
                Some("github"),
                Some("https://github.com/o/r/issues/1"),
                Some("done")
            ),
            None
        );
    }

    #[test]
    fn tracker_id_for_uses_identifier_for_linear_url_for_github() {
        assert_eq!(
            tracker_id_for("github", "https://github.com/o/r/issues/1", Some("ENG-9")),
            "https://github.com/o/r/issues/1"
        );
        assert_eq!(
            tracker_id_for("linear", "https://linear.app/x", Some("ENG-9")),
            "ENG-9"
        );
        // Linear with no persisted identifier falls back to the URL string.
        assert_eq!(tracker_id_for("linear", "ENG-42", None), "ENG-42");
    }

    // ─── F3: the PR-open poller ─────────────────────────────────────────────

    #[test]
    fn parse_pr_list_takes_first_pr_or_none() {
        let pr = parse_pr_list(
            r#"[{"number":7,"state":"OPEN","isDraft":false,"url":"https://github.com/o/r/pull/7"}]"#,
        )
        .unwrap();
        assert_eq!(pr.number, 7);
        assert!(!pr.is_draft);
        assert_eq!(pr.state, "OPEN");
        assert_eq!(pr.url, "https://github.com/o/r/pull/7");
        // An empty array (no PR on the branch) and junk both read as None.
        assert_eq!(parse_pr_list("[]"), None);
        assert_eq!(parse_pr_list("not json"), None);
        assert_eq!(parse_pr_list(""), None);
    }

    #[test]
    fn poll_open_nondraft_pr_fires_inreview_but_draft_does_not() {
        let open = PrInfo {
            number: 7,
            is_draft: false,
            state: "OPEN".into(),
            url: "https://github.com/o/r/pull/7".into(),
        };
        // A non-draft PR on a not-yet-advanced worktree → InReview.
        assert_eq!(
            poll_pr_open_decision(None, &open),
            Some(TrackerPhase::InReview)
        );
        assert_eq!(
            poll_pr_open_decision(Some("in_progress"), &open),
            Some(TrackerPhase::InReview)
        );
        // Idempotent / no regress: already InReview, or already Done.
        assert_eq!(poll_pr_open_decision(Some("in_review"), &open), None);
        assert_eq!(poll_pr_open_decision(Some("done"), &open), None);
        // A DRAFT PR is never a trigger (spec open question 5).
        let draft = PrInfo {
            is_draft: true,
            ..open.clone()
        };
        assert_eq!(poll_pr_open_decision(None, &draft), None);
        assert_eq!(poll_pr_open_decision(Some("in_progress"), &draft), None);
    }

    #[test]
    fn pr_list_argv_shape() {
        assert_eq!(
            pr_list_argv("o/r", "feat/x"),
            [
                "pr",
                "list",
                "--head",
                "feat/x",
                "--repo",
                "o/r",
                "--json",
                "number,state,isDraft,url",
            ]
        );
    }

    #[cfg(unix)]
    fn write_fake_gh(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("gh-fake");
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// A fake `gh` returning a non-draft PR for the branch → `pr_list_via_gh`
    /// parses it (proves the argv + parse path, no env mutation).
    #[cfg(unix)]
    #[tokio::test]
    async fn pr_list_via_gh_returns_the_branch_pr() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"[{"number":42,"state":"OPEN","isDraft":false,"url":"https://github.com/o/r/pull/42"}]"#;
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\nprintf '%s' '{body}'\nexit 0\n"),
        );
        let result = pr_list_via_gh(script.to_str().unwrap(), "o/r", "feat/x").await;
        let pr = result
            .unwrap_or_else(|e| panic!("pr_list_via_gh errored: {e}"))
            .expect("a PR should parse");
        assert_eq!(pr.number, 42);
        assert!(!pr.is_draft);
    }

    /// F3 AC10: a `gh` that exits non-zero surfaces as `Err` (the poller logs +
    /// skips it) — the loop never halts.
    #[cfg(unix)]
    #[tokio::test]
    async fn pr_list_via_gh_failure_is_err_never_halts() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_fake_gh(
            dir.path(),
            "#!/bin/sh\necho 'boom: gh failed' >&2\nexit 1\n",
        );
        let result = pr_list_via_gh(script.to_str().unwrap(), "o/r", "feat/x").await;
        let err = result.expect_err("a gh failure is an Err the poller skips");
        assert!(err.contains("boom: gh failed"), "unexpected err: {err}");
    }

    // ─── F4: merge → Done ───────────────────────────────────────────────────

    #[test]
    fn parse_pr_view_detects_merge_by_state_or_merged_at() {
        // MERGED state.
        let v = parse_pr_view(r#"{"state":"MERGED","mergedAt":"2026-07-08T00:00:00Z"}"#).unwrap();
        assert!(v.merged);
        assert_eq!(v.state, "MERGED");
        // Open PR — not merged.
        let v = parse_pr_view(r#"{"state":"OPEN","mergedAt":null}"#).unwrap();
        assert!(!v.merged);
        // A non-null mergedAt with a lagging state still counts as merged.
        let v = parse_pr_view(r#"{"state":"CLOSED","mergedAt":"2026-07-08T00:00:00Z"}"#).unwrap();
        assert!(v.merged);
        // Closed-unmerged is NOT merged.
        let v = parse_pr_view(r#"{"state":"CLOSED","mergedAt":null}"#).unwrap();
        assert!(!v.merged);
        assert_eq!(parse_pr_view("junk"), None);
    }

    #[test]
    fn poll_merged_pr_fires_done_then_stops() {
        let merged = PrView {
            state: "MERGED".into(),
            merged: true,
        };
        // A merged PR on an InReview worktree → Done.
        assert_eq!(
            poll_pr_merge_decision(Some("in_review"), &merged),
            Some(TrackerPhase::Done)
        );
        assert_eq!(
            poll_pr_merge_decision(Some("in_progress"), &merged),
            Some(TrackerPhase::Done)
        );
        // Terminal-stop: once Done, a re-observed merge never re-fires.
        assert_eq!(poll_pr_merge_decision(Some("done"), &merged), None);
        // An un-merged PR is never a Done trigger.
        let open = PrView {
            state: "OPEN".into(),
            merged: false,
        };
        assert_eq!(poll_pr_merge_decision(Some("in_review"), &open), None);
    }

    #[test]
    fn pr_view_argv_shape() {
        assert_eq!(
            pr_view_argv("42", "o/r"),
            [
                "pr",
                "view",
                "42",
                "--repo",
                "o/r",
                "--json",
                "state,mergedAt",
            ]
        );
    }

    /// A fake `gh` reporting a merged PR → `pr_view_via_gh` parses `merged`.
    #[cfg(unix)]
    #[tokio::test]
    async fn pr_view_via_gh_reports_merge() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"state":"MERGED","mergedAt":"2026-07-08T00:00:00Z"}"#;
        let script = write_fake_gh(
            dir.path(),
            &format!("#!/bin/sh\nprintf '%s' '{body}'\nexit 0\n"),
        );
        let view = pr_view_via_gh(script.to_str().unwrap(), "o/r", 42)
            .await
            .unwrap_or_else(|e| panic!("pr_view_via_gh errored: {e}"));
        assert!(view.merged);

        // A gh failure surfaces as Err (poller logs + skips, never halts).
        let fail = write_fake_gh(dir.path(), "#!/bin/sh\necho 'boom' >&2\nexit 1\n");
        assert!(
            pr_view_via_gh(fail.to_str().unwrap(), "o/r", 42)
                .await
                .is_err()
        );
    }

    #[test]
    fn poll_tick_all_failed_gates_backoff() {
        assert!(
            !PollTick::default().all_failed(),
            "an empty tick is not a failure"
        );
        assert!(
            PollTick {
                attempted: 3,
                failed: 3
            }
            .all_failed()
        );
        assert!(
            !PollTick {
                attempted: 3,
                failed: 1
            }
            .all_failed()
        );
    }
}
