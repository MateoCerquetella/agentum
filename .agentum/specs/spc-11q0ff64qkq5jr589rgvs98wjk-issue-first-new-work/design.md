# Architecture — Spec 025: Issue-first New Work

- **Spec:** `025-issue-first-new-work`
- **Phase:** Architect
- **Date:** 2026-07-22
- **Gate:** PASS

## 1. Current-state findings

1. `CreateWorkspaceWizard` already is the sole New Workspace front door and
   delegates creation to `useComposerState::submitQuick`. Step 3 currently mixes
   an existing-issue list, an independently submitted Create issue panel, agent
   choice, worktree name, and a `Start gated run` checkbox
   (`CreateWorkspaceWizard.tsx:1314-1505,1691-2100`).
2. The Create issue panel files immediately—its own button and title-field Enter
   call `onCreateIssueSubmit` (`CreateWorkspaceWizard.tsx:1946-1952,2018-2031,
   2084-2096`). Therefore issue creation is not owned by the wizard's final
   action and cannot participate in one progress/retry story.
3. `handleCreateIssueSubmit` already performs the authoritative GitHub create,
   constructs the complete `LinkedWorkItemSummary`, applies the linked-item
   state, and preserves the drafted body as linked context
   (`useComposerState.ts:1520-1599`). It currently returns `void` and clears the
   draft, which prevents a caller from directly continuing with the exact
   returned identity in the same React turn.
4. `submitQuick` owns setup policy, worktree creation, tracker-bind persistence,
   spec/run side effects, and workspace opening (`useComposerState.ts:2633-2808`).
   It reads linked-item state from its render closure and holds no checkpoint;
   retry after a post-worktree failure would attempt another worktree.
5. Autopilot's server primitive is already correct: `POST
   /api/harness/start-work` serializes per-process starts, converges an existing
   spec, plans, writes run knobs, registers, claims, and lets Harness drive own
   the only spawn (`routes/harness.rs:576-718`). Do not replace it.
6. Manual preparation's primitive, `POST /api/harness/spec-from-issue`, writes
   the right spec but deliberately returns 400 when it already exists
   (`routes/harness.rs:343-397`). A retry-safe manual path needs an opt-in
   converge flag; globally changing its never-overwrite default would weaken an
   established API contract.
7. `scaffoldSpec` is generic composer state, default false, and the wizard does
   not expose its checkbox. `submitQuick` still branches through it when the
   gated toggle is off (`useComposerState.ts:596-604,2751-2763`), which is why a
   normal wizard-created issue worktree has no spec.
8. Existing pure model tests run without a DOM. The new orchestration decisions
   should follow that precedent rather than add a browser-like test environment.

## 2. Architectural decisions

### A1 — Keep authoritative operations separate; compose a resumable UI saga

Do not add a new server endpoint spanning GitHub issue creation, git worktree
creation, filesystem writes, and agent launch. Those operations cross external
and local durability boundaries and cannot be rolled back transactionally.

Add a small React-free coordinator/model under
`components/new-workspace/new-work-launch-model.ts`. It owns:

- `WorkSource = 'new' | 'existing'`;
- `ExecutionMode = 'autopilot' | 'manual'`;
- ordered stages `issue → worktree → spec → run`;
- an in-memory checkpoint containing a confirmed linked issue and the full
  `createWorktree` result;
- stage status/copy and eligibility derivation;
- contextual primary label and retry position.

`CreateWorkspaceWizard` holds this model state for the modal lifetime and passes
the checkpoint into `submitQuick`. Server/store APIs remain the authority for
each completed stage. Closing the wizard discards the checkpoint, as explicitly
allowed by the spec's non-goal; it never deletes the already-created issue or
worktree.

### A2 — Return the created issue and pass it as a submit override

Widen `ComposerCardProps.onCreateIssueSubmit` from `() => void` to
`() => Promise<LinkedWorkItemSummary | null>`. `handleCreateIssueSubmit` keeps
applying normal composer state but also returns the confirmed summary on success
and `null` on failure. Existing callers may continue using `void
onCreateIssueSubmit()`.

Widen `submitQuick` with one optional `QuickSubmitOptions` object:

```ts
type QuickSubmitOptions = {
  linkedWorkItem?: LinkedWorkItemSummary
  executionMode?: 'autopilot' | 'manual'
  checkpoint?: NewWorkCheckpoint
  onCheckpoint?: (next: NewWorkCheckpoint) => void
  onProgress?: (stage: NewWorkStage, status: NewWorkStageStatus) => void
}
```

The explicit linked-item override wins over closure state for issue number,
title, name derivation, tracker coordinates, spec/run gating, and prompt context.
This prevents a stale React closure between `await create issue` and worktree
creation. Calls without options preserve current behavior for non-wizard
surfaces.

### A3 — Checkpoint immediately after each irreversible success

For New issue, the wizard calls `onCreateIssueSubmit()` only from its final
handler, then checkpoints the returned summary before asking `submitQuick` to
continue. For Existing issue, the selected summary is the initial issue
checkpoint and no create call occurs.

Inside `submitQuick`, after `createWorktree` returns, publish its full result to
`onCheckpoint` before metadata/spec/run calls. On retry, a matching checkpoint
skips `createWorktree` and reuses its `worktree`, `setup`, and `defaultTabs`.

Once an issue has been filed, lock project/source/issue draft fields. Once a
worktree exists, lock branch/name/agent/execution fields. The user may Retry,
open the surviving worktree, or close; they may not mutate the inputs beneath a
durable checkpoint. This avoids needing a fragile fingerprint comparison.

### A4 — Autopilot is strict ownership; manual preparation is idempotent

For `executionMode: 'autopilot'`, call the existing `startGatedWork` directly
through a strict helper that returns ownership or throws. Do not use today's
`maybeStartGatedRun` fallback behavior, which catches an error and opens a plain
session. If `planned === 0` and no run already owns the worktree, keep the wizard
open at the Run stage with an actionable error. Only confirmed ownership calls
`openCreatedWorkspace({ gatedRun: true })`.

For `executionMode: 'manual'` and an eligible local GitHub issue, call
`scaffoldSpecFromIssue({ plan: false, converge: true })`, checkpoint Spec done,
then use the unchanged `openCreatedWorkspace({ gatedRun: false })` path. Planning
is deferred until later adoption by `start-work`; manual preparation does not
move tracker status merely for opening an agent.

### A5 — Add opt-in converge semantics to spec-from-issue

Extend `SpecFromIssueRequest` with `converge: bool` (`serde(default)`) and add
`spec_existed` to its response. Thread the flag to `ensure_spec_and_plan`.

- absent/false: current 400-on-existing behavior remains byte-compatible;
- true: retain the existing spec, never overwrite it, and return success;
- `plan:false, converge:true`: manual preparation/retry path;
- `plan:true, converge:true`: supported by the shared core but not required by
  this UI path.

Extend `scaffoldSpecFromIssue` with the optional camelCase `converge` field.
This is a narrow API widening, not a new orchestration surface.

### A6 — Make issue source and execution outcome structural UI choices

Replace the step-3 tracker mixture with two segmented choices:

1. **Work source:** New issue (default when no issue was seeded) / Existing
   issue (default when modal data includes one).
2. **Execution:** SDD Autopilot / Open manually.

New issue renders the existing title/body/AI-draft controls and GitHub labels,
but no Create button and no title-Enter filing. Existing issue renders the
current project-scoped picker. Linear creation stays available in its existing
project intake surfaces but is not offered by this local-GitHub slice.

`SDD Autopilot` defaults only when the pure eligibility result is eligible. Its
description reads `PM → Architect → Build → Verify → Review`. Manual remains
selectable. Internal words `Harness`, `scaffold`, and `gated run` are removed
from this wizard's visible primary copy, not from APIs, code identifiers, logs,
or advanced surfaces.

### A7 — Eligibility is derived once and cannot silently change execution

Add `deriveNewWorkEligibility` to the pure model. Autopilot requires:

- local git repo;
- a GitHub issue source (staged New issue or parsed Existing issue URL);
- an installed selected agent;
- no connection/setup blocker already exposed by the composer.

It returns a discriminated result with a user-facing reason for `remote-repo`,
`non-git`, `non-github-issue`, `agent-unavailable`, or existing setup/connect
blockers. The UI renders that reason before submission and disables Autopilot.
Manual follows existing compatibility: local GitHub gets mandatory spec
preparation; other sources may create/open through their current manual path but
are explicitly labeled `Manual only · no generated SDD spec`. An unavailable
selected agent blocks launch until another installed agent is chosen; it is not
silently replaced.

### A8 — Keep generic composer behavior compatible

Do not globally delete `scaffoldSpec` or `startGatedRun` state in this slice:
other composer/Tasks entry points still consume those seams. Remove them only
from the New Work wizard presentation and bypass them when explicit
`executionMode` is supplied. Unoptioned `submitQuick` retains legacy semantics.
This keeps the change incremental and prevents an unrelated full-composer
regression.

## 3. Control flow

```text
Final primary action
  |
  +-- New issue, no issue checkpoint
  |     createGithubIssue
  |       success -> apply linked item + checkpoint Issue
  |       failure -> Issue error; stop
  |
  +-- Existing issue / checkpointed issue
  |     use selected confirmed summary
  |
  +-- no worktree checkpoint
  |     submitQuick -> existing setup checks -> createWorktree
  |       success -> checkpoint full creation result immediately
  |       failure -> Worktree error; stop
  |
  +-- Autopilot
  |     start-work -> ownership confirmed
  |       success -> Spec done + Run done -> open gated surface
  |       failure/no features -> Run error; keep wizard/worktree
  |
  `-- Manual
        spec-from-issue(plan=false, converge=true)
          success -> Spec done -> open one plain agent -> Run done
          failure -> Spec error; keep wizard/worktree
```

On Retry, completed branches are skipped from the in-memory checkpoint. No
compensating delete exists for GitHub issues or worktrees.

## 4. Files and seams

### Desktop UI

- `components/new-workspace/new-work-launch-model.ts` — new enums,
  checkpoint/stage types, reducer/derivations, eligibility, labels, retry step.
- `components/new-workspace/new-work-launch-model.test.ts` — pure state and
  compatibility matrix.
- `components/new-workspace/CreateWorkspaceWizard.tsx` — segmented source and
  execution cards, staged New issue editor, progress/retry surface, final
  coordinator, checkpoint locking.
- `components/new-workspace/create-workspace-wizard-model.ts` + `.test.ts` —
  contextual primary label/disabled state if these general wizard derivations
  remain outside the launch model; do not duplicate label logic.
- `hooks/useComposerState.ts` — return created summary; accept explicit quick
  submit options; publish/reuse worktree checkpoint; strict Autopilot/manual
  branches; preserve unoptioned legacy behavior.
- `runtime/github-issue-client.ts` + its existing/new focused test — optional
  `converge` request field and `specExisted` response.

### Server

- `crates/agentum-server/src/routes/harness.rs` — `converge` request field,
  `spec_existed` response field, route threading, and focused compatibility
  tests around false/true behavior. `start_work` and Harness drive stay
  otherwise unchanged.

No core/store schema, worktree registry wire, tmux, executor adapter, event bus,
or streaming changes are required.

## 5. Race and error handling

- Double-click/Enter is gated by the existing `creating`/busy state plus the
  launch model's active stage; both call the same final handler.
- GitHub create success is accepted only from the server response. The summary
  is checkpointed before React state or later work begins.
- If a late create response arrives after modal unmount, the component's mounted
  guard suppresses UI continuation; the issue remains real and is never deleted.
- Worktree success is checkpointed before metadata/spec calls. Retrying cannot
  call `createWorktree` again in the same modal lifetime.
- Autopilot never opens manual merely because `start-work` fails. An already
  running response counts as ownership; zero planned features without an owner
  is an error.
- Manual spec retries opt into converge and keep human edits byte-for-byte.
- Closing after a partial success is honest: the issue/worktree remain visible;
  durable cross-reopen recovery is deferred by PM scope.
- Changing project/source inputs is disabled after the first durable checkpoint,
  so an old issue/worktree cannot be attached to new form values.

## 6. Build order

1. **F1 — deferred-new-issue-intent:** pure launch model, staged New/Existing UI,
   return confirmed issue summary, contextual CTA, no early Create/Enter filing.
2. **F2 — issue-backed-spec-invariant:** optional converge server/client wire,
   mandatory manual preparation, explicit Autopilot mode, legacy compatibility.
3. **F3 — explicit-execution-and-recovery:** worktree checkpoint reuse, ordered
   progress, strict no-fallback ownership, field locking, surviving-worktree
   recovery presentation.

F2 depends on F1's explicit source/mode. F3 depends on both because checkpoints
cover the new issue and the mode-specific post-worktree stage.

## 7. Acceptance-criterion mapping

| AC | Implementation seam | Verification |
|---|---|---|
| 1 | source union + wizard segmented control/staged editor | model tests + rendered browser QA |
| 2 | returned issue summary + linked override + contextual CTA | fake create call count and new/existing branch tests |
| 3 | execution union + eligibility/default/copy derivations | pure matrix + DOM/browser text check |
| 4 | explicit mode branch; manual required scaffold; Autopilot start-work | fake stage-order tests + filesystem QA |
| 5 | strict `startGatedWork`; `openCreatedWorkspace(gatedRun:true)` only on ownership | ownership/no-fallback unit tests + single-session QA |
| 6 | `plan:false, converge:true`; unchanged manual activation | Rust converge tests + manual run QA |
| 7 | issue/worktree checkpoints + stage reducer/progress | retry call-count tests + forced-failure QA |
| 8 | discriminated eligibility + locked explicit mode | compatibility matrix + pre-submit reason QA |

## 8. Verification strategy

### Focused unit gates

- `bunx vitest run` on the new launch-model test plus existing wizard,
  create-issue-intent, issue-side-effect, gated-run-ownership, and
  open-created-workspace tests.
- New fake-dependency coordinator tests assert exact call order and call counts:
  `create issue ×1 → create worktree ×1 → prepare/start`; retry after each
  injected failure never repeats a completed irreversible call.
- `cargo test -p agentum-server --lib routes::harness::tests` covers default
  never-overwrite plus opt-in converge, `plan:false`, start-work adoption, and
  human-edited spec preservation.
- `npm run build --prefix crates/agentum-desktop/ui` and `git diff --check`.

### Browser QA

Use a scratch local GitHub repo and the installed desktop app:

1. New issue draft creates nothing before final submit.
2. Autopilot produces exactly one issue/worktree/spec and one Harness-owned
   agent; progress is ordered and internal terminology absent.
3. Manual produces exactly one issue/worktree/spec and one plain agent, with no
   active Harness driver.
4. Force failures after issue and worktree creation; Retry creates neither a
   duplicate issue nor a duplicate worktree.
5. Remote, non-GitHub, non-git, and unavailable-agent selections show their
   exact pre-submit reason and never silently change execution mode.

## 9. Architect gate

- Every AC maps to a named seam and verification method: PASS.
- All product choices are resolved by PM defaults: PASS.
- Existing create/worktree/spec/start/spawn primitives are reused: PASS.
- One launch path, gate integrity, tracker metadata, and human-edited specs are
  preserved: PASS.
- No migration or destructive operation is required: PASS.

Verdict: **PASS → Developer**.
