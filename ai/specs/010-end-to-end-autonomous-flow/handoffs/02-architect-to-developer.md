# Handoff 02 — Architect → Developer

- **Spec:** 010-end-to-end-autonomous-flow
- **Date:** 2026-07-06
- **From:** Architect (autonomous /sdd-loop iteration 2)
- **To:** Developer
- **Artifact:** `ai/specs/010-end-to-end-autonomous-flow/architecture.md`
  (nine calls resolved; per-feature build/test plan §8)

## Gate result

Architect gate: **PASS** — D1–D8 honored (self-check §-mapped); every named
call resolved with grounded rationale (§7 items 1–9); invariants intact
(zero call-site edits verified feasible against the real arms; label fns
byte-identical; best-effort `Ok`-never-`Err` extended; no spawn-path /
streaming / label-canon changes); citations line-verified at `388eaa66` and
re-checked after the mid-phase merge of origin/develop v0.59.0 (base now
`664ee365` — all four seam anchors unchanged); build/test plan gateable;
no scope creep (the id cache and the state-only `.gitignore` rewrite are
both forced by the spec's own constraints and argued in §6/§7).

## Environment note (do this first)

The worktree is on `664ee365` = origin/develop (v0.59.0) merged. Before
building, `git fetch origin && git merge-base --is-ancestor origin/develop HEAD || git merge origin/develop`
— develop moves fast today (v0.58.3 and v0.59.0 both landed while this spec
was in flight). Re-locate any line anchor you depend on before editing
(the 004 lesson).

## Build order + discipline

- **F1 → F2 → F3 (D6); inside F2 build the arm hook LAST**: pure builders +
  `board_write_with` + its fake-gh suite first, then the two-line hooks into
  `task_sink.rs`'s github arms — run the full existing label suite
  immediately after and confirm **zero test-file diffs** (AC 8 is
  "unmodified", not just "green").
- **First thing to write:** the pure fuzzy mapper against AC 2's four
  fixtures (`resolve_status_mapping` + `normalize` + the disjointness pin) —
  it is the contract everything else (constructor, discover route, editor
  pre-selection, D5 fallback hints) consumes, and it is pure (fastest
  feedback).
- **One gated slice per feature** (`010-f1-board-bind`, `010-f2-board-drive`,
  `010-f3-workspace-provision`); commit per slice per the repo rules (no
  AI-attribution trailers).

## Sacred boundaries (do not re-litigate)

- `github_transition_with` (:621) and `github_mark_blocked_with` (:654)
  byte-identical; all four seam call sites untouched (`drive.rs:388` wrapper,
  `board_goals.rs:605`, `routes/harness.rs:425`, `mcp.rs:1201`).
- `TrackerPhase` four variants (`BoardPhase` is projects-local);
  `TransitionResult` gains no variant — board failures fold into
  `Skipped(reason)` strings (docstring widened, doc-only).
- `useComposerState.ts` reached via props only; `isGoalStepReady` /
  `GoalStepInputs` untouched (template mode produces the repoId before
  `onContinue`).
- Desktop `gh.rs` write stubs stay dead; no `is_public` additions.
- Provision writes its own 5-label ensure loop over `pub(crate)`-widened
  builders instead of refactoring the transition's pinned 4-ensure sequence.
- **Hermeticity:** read the binding only AFTER the URL parse in both arms
  (the `GithubStateMap::from_env()`-placement comment at :744 explains why);
  test IO-adjacent logic via explicit `program`/`bindings_path` injection,
  never env mutation.

## Test-first items

1. The AC-7 fake-gh "board fails, transition still Ok(Skipped-note)" test.
2. The F3 run-twice test — write it BEFORE implementing the commit step (it
   defines the porcelain-empty / no-second-project mechanics).
3. The unbound-is-byte-identical seam test (binding `None` ⇒ today's exact
   5-invocation log).

## Deviation risks to watch

1. The `.gitignore` rewrite in the F3 commit path must keep
   `feature_list.json`/`handoff.md`/`qa/` ignored — committing engine-written
   state re-imports the worktree-noise problem `types.rs:715–723` exists to
   prevent.
2. `gh project create --format json` output parsing — verify field names
   against a real `gh` once and freeze them in the fixture.
3. The D4 template repo must be marked "template" on GitHub for `--template`
   to work — surface `gh`'s stderr verbatim if not.
4. The add-repo registration action for template mode needs a quick trace
   from `openModal('add-repo')`'s submit — reuse it, don't build a parallel
   registration path.

## Key files

`crates/agentum-server/src/github_projects.rs` (new),
`crates/agentum-server/src/routes/github_projects.rs` (new),
`crates/agentum-server/src/routes/provision.rs` (new),
`crates/agentum-server/src/task_sink.rs` (two arm hooks + one new private fn
+ two `pub(crate)` widenings), `crates/agentum-server/src/lib.rs` (two
`.merge`s), `crates/agentum-desktop/ui/src/{runtime/github-projects-client.ts,
lib/github-projects-binding.ts, lib/workspace-provision-step.ts,
lib/workspace-goal-step.ts, components/github-projects/ProjectBindingEditor.tsx,
components/NewWorkspaceProvisionStep.tsx, components/NewWorkspaceGoalStep.tsx,
components/NewWorkspaceComposerModal.tsx, components/settings/IntegrationsPane.tsx}`.

## Expected developer artifact

Code + tests per `architecture.md` §8, one gated slice per feature, plus
`tasks.md` tracking the slices (prior specs' shape) and updated
`.harness`-gateable state; then `handoffs/03-developer-to-tester.md`.
