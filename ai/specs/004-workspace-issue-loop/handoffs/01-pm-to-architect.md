# Handoff 01 — PM → Architect

- **Spec:** 004-workspace-issue-loop
- **Date:** 2026-07-01
- **From:** PM (autonomous /sdd-loop iteration 1)
- **To:** Architect
- **Artifact:** `ai/specs/004-workspace-issue-loop/spec.md` (PM-gated; decisions D1–D5 locked)

## Gate result

PM gate (`ai/skills/validate_handoff.md`): **PASS** — all nine items green after
edits. Load-bearing code cites spot-verified against the current tree
(`task_sink.rs:278-282` no-op, `worktrees.rs:249-260/:351-353` dropped metadata,
`harness/types.rs:16/:685` scaffold helpers, `git_fs.rs:90` `gh_in_dir`).

## Decisions locked (see spec "Decisions (PM-locked)")

D1 `Done` = label-only (never auto-close). D2 writes via `gh` CLI. D3 canonical
labels `status/todo|in-progress|ready-to-test|done`, ensure-created, exactly one
per issue. D4 one spec, build order F1 status-transition → F2 worktree-metadata
→ F3 composer-create → F4 spec-scaffold. D5 scaffold opt-in, off by default.

## Material PM finding

AC 4 as originally drafted ("zero changes to `harness/drive.rs`") was
**unsatisfiable**: `transition_tracker` (`drive.rs:321`) passes only
`feature.id` into `apply_tracker_transition`, but the GitHub arm needs the repo
slug. AC 4 now permits exactly one mechanical widening — thread
`feature.tracker_url` through the seam — while forbidding any change to control
flow, transition points, or autonomy mechanics.

## What to blueprint (in D4 order)

1. **F1 (riskiest, do first):** the GitHub arm of `apply_tracker_transition`
   (`task_sink.rs:278-282`). Decide the seam widening (an `Option<&str>
   tracker_url` param threaded from `drive.rs:321` — the only permitted
   drive.rs touch) and where the repo-slug parse lives (mirror
   `parse_gh_issue_url`, `task_sink.rs:322`). Design the `gh` invocations as
   pure, unit-testable argv builders: label ensure-create (idempotent, fixed
   colors, `--repo <slug>` so cwd is irrelevant) + set-one/remove-others label
   semantics. Every `gh` failure maps to `Skipped(reason)` — the best-effort
   contract (`task_sink.rs:212-214`) is sacred; AC 5 pins it.
2. **F2:** `CreateBody` widening + registry persistence
   (`routes/worktrees.rs:249-260/:351-353`), serde-compatible with old clients.
3. **F3:** composer create-issue flow — key design question is sequencing (issue
   created + rendered before worktree creation, clean failure mode, no orphan
   state) and whether to reuse the Tasks-page create path or add a thin endpoint
   over `TaskSink::Github::create_feature`.
4. **F4:** deterministic issue-body→spec transform + HTTP seam over the existing
   `scaffold_harness`/`plan_from_spec` helpers (`harness/types.rs`) — decide
   param-on-create vs standalone `POST`; preserve untrusted-content containment
   for the issue body.

Also confirm `board_sync.rs:456-478` never strips `status/*` labels (D1 removes
the open/closed race, but not label interference).

## Expected architect artifact

`ai/specs/004-workspace-issue-loop/architecture.md` — boundaries, seam
signatures, tradeoffs, risks, per-feature build/test plan (matching prior specs'
`architecture.md` shape).
