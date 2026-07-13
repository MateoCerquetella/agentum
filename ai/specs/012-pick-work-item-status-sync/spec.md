# Spec 012 — Pick the work item, sync its status through the session lifecycle

- **Number:** 012
- **Status:** PM              <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-desktop/ui` (New Workspace picker) + `crates/agentum-server` (session-start hook, `TrackerPhase::InReview`, PR/merge poller) — building on spec 010's Projects v2 binding + write path.
- **Author:** Mateo (via /sdd-spec)
- **Date:** 2026-07-08

> **Grounding caveat (read first).** This spec was researched in the
> `new-chat-refresh` worktree, which is **59 commits behind `origin/develop`
> and is missing specs 009 and 010**. Spec 010 (*End-to-End Autonomous Flow*,
> RELEASED v0.60.0) already shipped the **per-repo Projects v2 binding**
> (`{project_id, status_field_id, status_mapping}`), the
> `updateProjectV2ItemFieldValue` **Project-column write inside
> `apply_tracker_transition`**, and the `done_closes_issue` knob — none of which
> are visible in this worktree. Every file:line below is approximate and the
> Architect **must re-ground on fresh `origin/develop`**. Where this spec says
> "build", first confirm 010 didn't already build it — **reuse 010 over
> rebuild** is a hard rule here (010 is this spec's foundation, not its rival).

## Problem

Mateo creates a workspace, links it to a GitHub Project, sets an agent
working — and the board never moves. Nothing updates because the workspace links
the **Project**, but never picks the **actual work item**: there's no issue
picker in the New Workspace flow (you can only paste a URL or create a brand-new
issue), so a plain workspace is bound to nothing a transition could target. And
even when an issue *is* linked, status only ever moves *inside a gated harness
run* — a normal "create a workspace and code" session drives no status at all,
and there is no notion of **In Review on PR** or **Done on merge**. So the loop
Mateo wants — pick the card, watch it march Todo → In Progress → In Review →
Done as he works — silently doesn't happen; he still drags cards by hand.

## Goal

Let the operator **pick the actual work item** from the linked GitHub Project
when creating a workspace, then have agentum **move that item automatically as
the workspace's session progresses** — In Progress when the agent starts, In
Review when a PR opens, Done when it merges — writing **both** the `status/*`
labels and the Project's Status column (reusing spec 010's binding), for a
**plain workspace** as well as a gated run.

## Users / personas

- **Mateo (solo operator), at two moments:**
  1. *Creating a workspace* — he opens New Workspace, wants to grab the specific
     card he's about to work from his Project board, not retype its URL or mint
     a duplicate issue. Picking it should bind this workspace to that item.
  2. *Working the session* — as his agent codes, opens a PR, and merges, he
     wants the card to walk across his real GitHub Projects board on its own
     (In Progress → In Review → Done), with no hand-dragging — whether or not he
     kicked off a gated harness run.

## Acceptance criteria

Ordered slices F1 → F4; each criterion independently gateable. Every write-back
criterion assumes the repo is **bound** per spec 010 (a Project + status-field
mapping exists); an unbound repo degrades to label-only exactly as 010 defines.

**F1 — Pick & bind the work item**

1. The New Workspace flow **renders an issue picker** listing the linked
   Project's open **issues** (PRs excluded), sourced from the existing read
   path (`gh_get_project_view_table` → the `github` slice `projectViewCache`),
   scoped to `settings.githubProjects.activeProject`
   (`shared/github-project-types.ts`). The picker is reachable from the
   **default wizard front door** (`CreateWorkspaceWizard.tsx` step 3, the
   currently display-only Tracker section, `:890-929`) — which today binds
   nothing (`initialLinkedWorkItem: null`, `enableIssueAutomation: false`).
2. **Selecting an item binds it:** it flows through `applyLinkedWorkItem(...)`
   (`useComposerState.ts:~1312`, the one existing attach seam) so the chosen
   issue becomes the workspace's `linkedWorkItem`, and on create is
   **persisted on the worktree** via `createWorktree(...)` → the registry's
   `linked_issue` (`routes/worktrees.rs` `struct Worktree:~46-63`) plus enough
   coordinates to reconstruct the tracker URL and the Project item (issue
   number + owner/repo slug; the Project **item id** from the picked row, so a
   later Projects write needn't re-resolve it).
3. **Picking is optional and non-fatal:** a workspace created without picking
   still creates and launches exactly as today; an empty/unreachable Project
   (no `activeProject`, `gh` unavailable, remote repo) shows an honest empty
   state and never blocks the step (mirrors `deriveWizardTracker`'s fail-closed
   rule).
4. **Pure model covered:** the picker's item-list derivation (from a project
   view) and the bind payload (from a selected row) live in a jsdom-free model
   module with unit tests; `bunx vitest run` for it is green.

**F2 — In Progress on session start (any bound session)**

5. A **session-start hook** on the one launch path
   (`routes::sessions::spawn_agent_into_pane`, or the session-lifecycle event
   bus) fires when an agent session starts in a worktree **bound to a tracker
   item**: agentum resolves `(provider, tracker_url)` from the worktree's
   `linked_issue`/`linked_pr`/`linked_linear_issue` + the repo remote slug and
   calls `apply_tracker_transition(..., TrackerPhase::InProgress)` — which,
   per spec 010, writes **both** the `status/*` label and the Project Status
   option for a bound repo.
6. This fires for a **plain workspace with no gated harness run** — a
   create-then-code session moves the card to In Progress. It is **idempotent**:
   re-starting a session in an already-In-Progress workspace re-issues the same
   transition (labels/option-set are idempotent) with no error, no duplicate,
   and it converges with the harness's own InProgress transition (same phase) —
   asserted no-thrash by a fake-`gh` test.
7. **Best-effort, never-halt:** a failed transition logs (tracing +
   `HarnessEvent::Log` where a run context exists) and **never blocks session
   start** — the extended tracker contract (010 AC 7). A session with no bound
   item is a silent no-op.

**F3 — In Review on PR open**

8. A new **`TrackerPhase::InReview`** variant (`task_sink.rs:~218`) with
   per-provider mappings: `status/in-review` label (via `GithubStateMap`), a
   Linear "In Review" state (via `LinearStateMap`, configurable, fail-closed
   skip if the team has no such state), and an **InReview entry added to spec
   010's `status_mapping`** — resolved by the same single-select option-ID
   discovery, with 010's nearest-earlier-phase fallback (`InReview →
   InProgress`) when the board has no In-Review-like column.
9. A new **PR-open detector** — a bounded background poll of
   `gh pr list --head <branch> --json number,state,isDraft,url` for each bound
   workspace's branch (there are **no inbound webhooks** —
   `routes/board_sync.rs:14` — so poll is the sanctioned model). When a
   non-draft PR is first seen for a bound branch, agentum **persists the PR
   number** onto the worktree (`linked_pr`) and fires
   `apply_tracker_transition(..., TrackerPhase::InReview)`. Asserted by a
   fake-`gh` returning a PR for the branch → an InReview transition + persisted
   `linked_pr`.
10. The poll is **bounded and best-effort:** it backs off, never blocks a
    session or a gate, and a `gh` failure logs without halting — asserted by a
    fake-`gh`-exits-nonzero test.

**F4 — Done on merge**

11. The same poller detects **merge** (`gh pr view <n> --json state,mergedAt` →
    `MERGED`) and fires `apply_tracker_transition(..., TrackerPhase::Done)` for
    the bound item — moving both the `status/done` label and the Project Status
    option to Done, and (per 010's `done_closes_issue`) closing the issue when
    that knob is on (the PR's own `Closes #N` also closes it). Asserted by a
    fake-`gh` returning a merged PR → a Done transition.
12. **Terminal:** once Done fires for a workspace's item the poller stops
    polling that PR (no infinite re-transition); a reopened/re-merged PR is out
    of scope (see Open questions). The `cargo test -p agentum-server --lib`
    suite (fake-`gh` transition + poll tests) and the new-model `bunx vitest
    run` are green.

## Scope & non-goals (YAGNI)

- **In:** the four slices above — an issue picker in the New Workspace flow that
  binds the chosen Project item to the workspace; a session-start → InProgress
  hook for any bound session; a new `InReview` phase; a `gh` PR-open/merge
  poller driving InReview/Done. GitHub Issues + Projects v2 only. Reuse of spec
  010's binding, Projects-column write, mapping surface, and `done_closes_issue`.
- **Out:**
  - **Rebuilding spec 010.** The Projects v2 Status-field write, per-repo
    binding `{project_id, status_field_id, status_mapping}`, the mapping edit
    surface, and issue-close-on-Done already exist (010 F1/F2) — this spec
    **reuses** them and only *extends* the mapping with `InReview`.
  - **Inbound webhooks / echo suppression** — poll only, one-way authoritative
    (010's stance; `board_sync.rs:14`).
  - **Reopened / draft-toggled / re-merged PRs, force-push edge cases** — v1
    treats the first non-draft PR as In Review and merge as terminal.
  - **Non-GitHub PR/MR detection** — Linear MR / GitLab MR lifecycle deferred;
    GitHub-first. (Linear *status* still moves via the existing `transition_issue`
    arm for the InProgress hook if the bound item is Linear, but PR detection is
    GitHub-only in v1.)
  - **Changing the harness gate semantics** — ReadyToTest still = unit-gate
    green inside a gated run; the InReview/Done-on-merge additions are the
    session/PR-lifecycle layer and coexist with the gated loop.
  - **Tracker configuration UI in the wizard** beyond 010's existing mapping
    surface.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- **Project item read path:** `gh_get_project_view_table` (Tauri command,
  `crates/agentum-desktop/src/commands/gh_projects.rs:~760`, paginates a
  project's issues/PRs with their Status field values) → the `github` slice
  `projectViewCache`. `settings.githubProjects.activeProject` +
  `ProjectPicker.tsx` already select the project.
- **Attach seam:** `applyLinkedWorkItem` (`useComposerState.ts:~1312`) is the
  single entrypoint that binds an existing work item; the reverse flow
  precedent is `launchWorkItemDirect` (`lib/launch-work-item-direct.ts:~193`,
  Project board → pre-linked workspace).
- **Persist on worktree:** `createWorktree(...)` (`store/slices/worktrees.ts`)
  → `api.worktrees.create` → registry `linked_issue`/`linked_pr`/
  `linked_linear_issue` (`routes/worktrees.rs`).
- **Write-back (the whole thing):** `apply_tracker_transition` /
  `apply_blocked_transition` (`task_sink.rs`) — real `gh` label writes + (010)
  the Projects `updateProjectV2ItemFieldValue` arm + `done_closes_issue`. Fake-
  `gh` subprocess test pattern already established there. Linear:
  `transition_issue` + `LinearStateMap` (`linear.rs`).
- **One launch path:** `routes::sessions::spawn_agent_into_pane` and the
  session-lifecycle event bus (the watchdog's `agent.*` events) — the hook plugs
  in here, not a special-case spawn.
- **Poll precedent:** `board_sync.rs`'s manual/poll model + the watchdog
  background-worker pattern (`agentum-watchdog`).

### Build new

- **Issue picker UI** in the wizard's Tracker section + pure list/bind helpers
  (jsdom-free model, in the `new-workspace` model seam).
- **Session-start → InProgress hook** — resolve `(provider, tracker_url)` from
  the worktree's linked fields + repo remote, then call the existing seam.
- **`TrackerPhase::InReview`** + its mappings (`status/in-review` label, Linear
  "In Review", the 010 `status_mapping` option entry) + fallback.
- **PR-open/merge poller** — a bounded background worker keyed on bound
  workspaces' branches; persists `linked_pr`; fires InReview then Done.
- **Worktree bind metadata** — persist the Project **item id** + issue
  URL/provider at pick time if `linked_issue` + remote can't reconstruct the
  Projects write target on its own (Architect to confirm against 010's read of
  `tracker_url`). Any new registry field stays **serde-alias-FREE** (spec 004
  lesson).

## Risks & invariants

- **Reuse-010-over-rebuild.** The single biggest risk is duplicating 010's
  Projects write / binding / close-on-Done because this worktree can't see them.
  Architect re-grounds on `origin/develop` first.
- **One launch path.** The InProgress hook must live at
  `spawn_agent_into_pane` / the lifecycle bus — never a bespoke spawn that
  bypasses YOLO translation, `pane_env`, or MCP wiring.
- **Idempotent, best-effort, never-halt.** Every transition (session-start,
  PR-open, merge) must be idempotent and non-blocking: a red label/board/Linear
  write logs and the session/gate proceeds. No transition may throw into the
  launch path or the poll loop. Converge session-InProgress with harness-
  InProgress (same phase) — no thrash.
- **Fail-closed binding.** An unparseable/missing remote or an empty Project
  yields **no bind and no transition**, never a fabricated or wrong-issue one
  (mirrors `deriveWizardTracker` / `BaseRefPicker`).
- **No webhooks → poll only.** Do not reintroduce push-snapshot polling of
  panes; the PR poll is a separate, bounded, backed-off `gh` loop.
- **InReview vs the "review" board column.** 010 maps `ReadyToTest → "review"`
  board column; adding `InReview` must not collide — reconcile the board-column
  and Project-option mapping (Open question).
- **Worktree registry serde-alias-FREE** (spec 004): if new bind fields are
  added, no serde aliases (they wipe on read).

## Harness wiring (the gate)

- **feature_list.json entries** (one shippable slice each):
  1. `pick-work-item` — issue picker over the active Project + bind on create
     (AC 1–4).
  2. `in-progress-on-start` — session-start → InProgress hook for any bound
     session (AC 5–7).
  3. `in-review-on-pr` — `TrackerPhase::InReview` + mappings + PR-open poll
     (AC 8–10).
  4. `done-on-merge` — merge detection → Done + terminal poll stop (AC 11–12).
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green (fake-`gh`
  transition + poll tests: InProgress idempotent/no-thrash, InReview on PR,
  Done on merge, `gh`-failure never halts) **AND** `bun run build --prefix
  crates/agentum-desktop/ui` succeeds **AND** `bunx vitest run` for the new
  picker model is green. No `tsc` gate (shared/* is a vite alias).
- **`qa.sh` asserts (browser + a real Projects board, human/qa runner, same
  class as 010 AC-11):** create a workspace → **pick an issue** from the linked
  Project → the card moves to **In Progress** when the agent starts → open a PR
  from the branch → card moves to **In Review** → merge the PR → card moves to
  **Done** and the issue closes — observed on a real GitHub Projects board with
  the mapped column names. Evidence = the issue's timeline (project-status +
  close events) + a demo-pass line in `ai/STATE.md`.

## Open questions (need a human/architect decision before build)

- **Poller placement + cadence.** Watchdog background worker vs a harness-
  adjacent poller vs a git-route-triggered check; how it enumerates "bound
  workspaces with an open branch"; default cadence (proposed 30–60 s). *Architect
  call; confirm cadence with Mateo.*
- **InReview board-column mapping.** 010 already uses the "review" board column
  for `ReadyToTest`. Does `InReview` get a **distinct** Project column / board
  column (Mateo's board convention), or does In-Review fold onto the existing
  one? *Needs Mateo's board layout.*
- **Bind granularity.** Does the InProgress hook fire on **any** agent session
  start in the bound worktree (including a plain terminal-tab agent), or only
  the first coding agent? Proposed v1: any agent session, idempotent. *Confirm.*
- **Project item id at pick time.** Persist the picked row's Project **item id**
  (cheap, already in the row) so the Projects write needs no re-resolve, or rely
  on 010's idempotent `addProjectV2ItemById`-by-content? *Architect, against
  010's actual `tracker_url` read.*
- **Draft PRs.** v1 = first **non-draft** PR → In Review (a draft PR does not
  trigger). Confirm that's the desired trigger point.
