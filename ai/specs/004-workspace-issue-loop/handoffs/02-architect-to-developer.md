# Handoff 02 — Architect → Developer

- **Spec:** 004-workspace-issue-loop
- **Date:** 2026-07-01
- **From:** Architect (autonomous /sdd-loop iteration 2)
- **To:** Developer
- **Artifacts:** `architecture.md` (complete blueprint), spec.md aligned to C4/C5

## Gate result

Architect gate: **PASS** — every AC has a design home (AC 3–5 → F1, AC 2 → F2,
AC 1 → F3, AC 6–7 → F4); seams line-verified pre-merge; invariants intact
(best-effort tracker, one launch path, auth layer, no polling); tradeoffs +
rejected alternatives stated; per-feature test plans map to the verify gate.

## ⚠️ Environment note (read first)

After line verification, `origin/develop` (+35 commits) was **merged into this
worktree** — including spec 003's `task_sink.rs`/`chat.rs` changes
(`NewFeature.labels`, `gh --label` on CREATE). The design stands; **re-locate
every `:line` cite before editing** (grep the named symbols). 003's
labels-on-create never removes labels, so F1's invariants are unaffected.

## Build order + first move

F1 → F2 → F3 → F4, each an independently gated slice
(`cargo test -p agentum-server --lib` + `npm run build --prefix
crates/agentum-desktop/ui`; commit per green slice).

**First failing test:** `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three`.

## Non-negotiables (from the blueprint)

1. GitHub arm returns `Ok(Skipped(reason))` for EVERY failure — never `Err`.
2. `drive.rs` diff = one line (thread `feature.tracker_url.as_deref()`);
   `board_goals.rs` = one line (`url.as_deref()` on the initial Todo).
3. Slug AND number parse from `tracker_url` (F4's N-features-per-issue makes
   `feature.id` unusable as the issue number).
4. Remove only the other THREE canonical labels — never `status/qa*` (C4).
5. NO serde alias on the registry `Worktree` struct (duplicate-field →
   `read_worktrees` wipes to `[]`). Alias only `CreateBody`.
6. F3 = new `POST /api/github/issues` (Tasks-page path is a local stub — C1).
7. F4 transform: `- [ ]` lines stay bare (prefixing breaks
   `derive_backlog_from_spec`); go through the `plan_from_spec` inner refactor,
   NOT `write_backlog_from_features`.
8. Subprocess tests: pass a fake-`gh` script path as the explicit `program`
   param — no env mutation, no lock.
9. No `is_public` additions; new routes ride `require_token`.
10. `cargo fmt --all` before each commit (CI is tag-gated but fmt-fail masks
    the rest); never `git add -A` (concurrent agents).

## Key files

`crates/agentum-server/src/task_sink.rs`, `harness/drive.rs`,
`routes/{board_goals,worktrees,github,harness}.rs`, `harness/types.rs`;
`crates/agentum-desktop/ui/src/tauri/worktrees.ts`,
`runtime/{server-worktree-client,github-issue-client}.ts`,
`hooks/useComposerState.ts`, `components/NewWorkspaceComposerModal.tsx`.

## Reviewer focus (carry forward)

The one-line drive.rs diff; `Ok(Skipped)`-never-`Err`; the C3 wire keys
(`linkedPR` on detected read path, alias on CreateBody only); traversal-proof
`issue_spec_id`; keep-existing spec semantics (409 on existing spec.md).
