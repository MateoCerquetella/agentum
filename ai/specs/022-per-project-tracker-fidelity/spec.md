---
tracker: none-yet   # recommend filing a new issue (see Open questions); related to https://github.com/MateoCerquetella/agentum/issues/360
---

# Spec 022 — Per-project tracker fidelity: show Status, keep the binding, open on GitHub

- **Number:** 022
- **Status:** In progress       <!-- Draft | PM | Architect | In progress | Done --> (PM ✓ · Architect ✓ · Developer: **B + C implemented + committed** `6c539d4b` on branch `feat/022-tracker-fidelity` (worktree `/Users/mateocerquetella/Developer/projects/agentum-022-tracker-fidelity`, based on origin/develop bb25a97d / v0.78.0). **A deferred** — design call, and the sidebar issue card ALREADY shows a Project-Status chip via spec 018/#365 (drift found during impl). NOT pushed / no PR / no browser-QA — human gates.)
- **Surface:** `crates/agentum-desktop/ui` (+ existing `crates/agentum-desktop/src/commands/shell.rs`; no new backend)
- **Author:** Mateo Cerquetella (drafted via `sdd-spec`)
- **Date:** 2026-07-17
- **Tracker:** _none filed yet — recommend a NEW issue (see Open questions). #360 (per-project tracker binding) shipped v0.78.0 and is CLOSED, so these follow-on fixes need their own ticket._
- **Code baseline:** `origin/develop` (this `cero` worktree is pinned to v0.57.0-era code; every `path:line` below is on `origin/develop`).

## Problem

Spec #360 made the GitHub tracker a **per-project** binding, but the binding fails
to surface in three places the user actually looks:

1. **The board hides Status.** In a project's **Tasks** tab the Projects-v2 board
   card shows title/number/repo/labels/assignees but **not the item's Status**
   (the single-select "Status" value). The user only infers status from which
   column a card sits in — the "STATUS of the project" they expect is invisible on
   the card itself.
2. **A new-workspace issue looks unassigned.** When the user files a GitHub issue
   from inside **Create New Workspace**, the resulting worktree row in the sidebar
   does not show the tracker/status chip for that issue — the workspace looks
   unassigned even though an issue was just created for it.
3. **"Open on GitHub" is dead.** The ↗ button on a worktree's linked-issue card
   does nothing when clicked, so there is no way to jump from the app to the issue
   on github.com.

## Goal

Make a project's tracker binding trustworthy end-to-end: its **Status** is visible
on every issue card, a workspace created with a new issue stays **linked** (issue +
tracker chip) on its sidebar row, and its issue is **reachable on GitHub** — for
local and SSH-remote projects alike.

## Users / personas

**Mateo (and any agentum operator)** running the desktop app: they bind a GitHub
Project to a project, spin up a workspace tied to a freshly-filed issue, and expect
the sidebar + hub board to reflect that issue's status and let them open it on
GitHub. Today all three moments quietly under-report the binding.

## Acceptance criteria

### A — Board card shows Status (UI-only)

1. In a project's Tasks tab rendering the Projects-v2 board, each
   `ProjectBoardCard` renders its **Status** value as a visible chip (option name +
   the option's color dot), read from `row.fieldValuesByFieldId[board.field.id]`,
   not merely implied by the card's column.
2. Each board card also renders the item's **issue type** (when present) and a
   relative **"updated X ago"** timestamp (`row.updatedAt`), reusing the existing
   color/format helpers — no new backend fields.
3. **(No regression)** The table view keeps rendering Status exactly as today
   (`ProjectCell.tsx` `SingleSelectCell`); the unbound coarse Kanban fallback is
   unchanged.

### B — New-workspace issue stays linked on the sidebar row

4. Filing a GitHub issue inside the Create New Workspace wizard and then creating
   the workspace **persists the worktree's tracker bind** (`trackerProvider` +
   `trackerUrl`) in addition to `linkedIssue`, so the new sidebar row shows both
   the issue number and a non-null tracker/status chip (`TrackerPhaseChip`).
5. The tracker bind is persisted on **both** the local (`api.worktrees.create`)
   and SSH/remote (`callRuntimeRpc('worktree.create')`) create paths — the local
   IPC adapter no longer strips `trackerProvider`/`trackerUrl`.
6. **(No regression)** The full-composer `submit` path still persists the tracker
   bind (already works for remote; now also works for local), and `linkedIssue`
   still renders as `#N` on the row as it does today.

### C — "Open on GitHub" actually opens

7. Clicking the **View on GitHub** (↗) button on a worktree's linked-issue card
   opens `issue.url` (the GitHub `html_url`) in the system browser via
   `api.shell.openUrl` — working for both local and SSH-remote repos. No dead
   `target="_blank"` anchor remains for that button.
8. **(No regression)** The sibling **Open in Agentum** and **Edit issue** buttons
   in the same row still work; no other `MetadataActionIcon` `href` caller loses
   true-anchor behavior it depends on.

## Scope & non-goals (YAGNI)

- **In:**
  - Board-card Status + issue-type + updated-time chips (UI-only; data already
    fetched).
  - Carry `trackerProvider`/`trackerUrl` through `submitQuick` and the local
    worktree IPC adapter so the wizard/local create persists the tracker bind.
  - Route the linked-issue ↗ button through `api.shell.openUrl`.
- **Out:**
  - **Milestone / reviewers / linked-PRs / sub-issue-progress** on the card — these
    are genuinely missing end-to-end (no GraphQL selection today) and are a
    separate full-stack change. Deferred.
  - Removing the sidebar **Board** entry / the per-project-binding rework itself —
    that is #360 proper, already shipped; this spec fixes where its binding leaks.
  - Redesigning the unbound coarse open/closed Kanban (`TaskKanbanBoard`).
  - Any change to the server binding/read layer, the binding data model, or GitLab
    parity.
  - Fixing the unrelated `linkedPr`/`linkedPR` casing quirk (note it; don't worsen).

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- **Board render path** — `project-hub/ProjectHubPage.tsx:239` (Tasks tab →
  `<TaskPage embedded>`), effect `:84-125` forces `githubMode:'project'` when the
  repo has a binding → `github-project/ProjectViewWrapper.tsx:846-859` branches
  `BOARD_LAYOUT → ProjectBoardView → ProjectBoardCard.tsx` vs table →
  `ProjectViewList → ProjectRow → ProjectCell.tsx`.
- **Status is already fetched** — desktop command
  `crates/agentum-desktop/src/commands/gh_projects.rs` `FIELD_VALUE_SELECTION:207-216`
  (single-select `{optionId name color}`), normalized `:381-388`, surfaced in
  `GitHubProjectRow.fieldValuesByFieldId` (`shared/github-project-types.ts:181`);
  the single-select value variant lives at `github-project-types.ts:133`.
- **Working Status/type renderers to imitate** — `ProjectCell.tsx` `SingleSelectCell`
  (`:88-95`) and the Type chip (`:314-322`); color helpers `SINGLE_SELECT_HEX` +
  `optionDotColor` (`ProjectBoardView.tsx:22-49`) and `singleSelectChipColors`
  (used at `ProjectCell.tsx:316`). The board group field id comes from
  `ProjectBoardView.tsx:66` (`boardColumns(table)` / `board.field`).
- **Tracker bind derivation** — `work-item-picker-model.ts:166-180`
  `deriveTrackerBindCoords(workItem) → {trackerProvider, trackerUrl}`. The full
  `submit` path already uses it: `useComposerState.ts:2470` + passes it at
  `:2494-2495`.
- **Server already accepts + persists + returns the bind** —
  `routes/worktrees.rs` `CreateBody` `:383-406` (`tracker_provider:404`,
  `tracker_url:406`), `create()` persists `:501-502`, `list()`/`detected_row`
  return `linkedIssue`/`trackerProvider`/`trackerUrl` (`:299`, `:912/:921/:922`).
  **No server change needed.**
- **Sidebar render** — `WorktreeCard.tsx:236-252` renders `#{linkedIssue}` +
  "Loading issue…"; the tracker chip is `WorktreeCard.tsx:608` →
  `WorktreeCardMeta.tsx:333` `<TrackerPhaseChip>` which returns **null when the
  bind is absent** (`TrackerPhaseChip.tsx:41`).
- **The correct "open external URL" helper** — `api.shell.openUrl(url)`
  (`tauri/shell.ts:14` → `shell_open_url`; Rust `commands/shell.rs:79`, registered
  `lib.rs:223`). Canonical working caller to imitate:
  `GitHubItemDialog.tsx:815` `onClick={() => api.shell.openUrl(workItem.url)}`.

### Build new

- Board-card Status/type/updated chips in `ProjectBoardCard.tsx` (footer ~`:110-160`).
- `submitQuick` (`useComposerState.ts:2602`, `createWorktree` call `:2681-2703`):
  compute `deriveTrackerBindCoords(submitLinkedWorkItem)` and pass
  `trackerProvider`/`trackerUrl` — mirroring `submit`. **Correct the false comment
  at `CreateWorkspaceWizard.tsx:258`** ("submitQuick persists the tracker bind").
- Forward `trackerProvider`/`trackerUrl` (and the GitLab issue/MR fields) in the
  local adapter `tauri/worktrees.ts:16-30` and `runtime/server-worktree-client.ts:26-38`
  so the local path matches the remote RPC set.
- Switch the **View on GitHub** button (`WorktreeCardMeta.tsx:313`) from the
  `href` anchor branch of `MetadataActionIcon` (`:117-163`, dead `target="_blank"`
  at `~:133`) to an `onClick={() => api.shell.openUrl(issue.url!)}` — either at the
  call site or by making the helper's `href` branch call `openUrl` (only after
  confirming no other `href` caller needs true-anchor semantics).

## Risks & invariants

- **Never reintroduce `target="_blank"` / `window.open`** — dead in the Tauri
  WKWebview (corroborated: `ChatPage.tsx:460-461`, `GhAuthErrorHelp.tsx:66`, and
  the regression test `right-sidebar/SourceControl.hosted-review-header-link.test.tsx:35`).
  All external navigation goes through `api.shell.openUrl`.
- **Positional-arg drift** — `createWorktree` is called positionally (16 args in
  `submitQuick`, 19 in `submit`). Adding the two tracker args must match the store
  slice signature (`store/slices/worktrees.ts:1014`, params `:1032-1033`) exactly,
  or the bind silently lands in the wrong slot. Prefer aligning `submitQuick` to
  the `submit` arg order verbatim.
- **Adapter field names must match server serde** — the added local-adapter fields
  serialize to the camelCase `CreateBody` names (`trackerProvider`/`trackerUrl`);
  a mismatch is silently dropped by `#[serde(default)]`.
- **UI-only card change stays UI-only** — do not touch `gh_projects.rs` /
  `routes/github_projects.rs` / `github_projects.rs` for criteria A1–A3.
- **Don't worsen** the unrelated `linkedPr`/`linkedPR` casing quirk in
  `worktrees.rs list()`.

## Harness wiring (the gate)

- **`feature_list.json` entries (3 increments, in order):**
  - `022-A-board-card-status` — Status/type/updated chips on `ProjectBoardCard`.
  - `022-B-wizard-tracker-bind` — persist tracker bind via `submitQuick` + local
    adapter.
  - `022-C-open-on-github` — route ↗ through `api.shell.openUrl`.
- **`verify.sh` asserts (unit gate):**
  - `npm run build --prefix crates/agentum-desktop/ui` is green (vite; note: not
    full `tsc` — `shared/*` is a vite alias).
  - `bunx vitest` for the pure/model + component suites, including: a
    `deriveTrackerBindCoords` unit around the `submitQuick` payload; a
    `ProjectBoardCard` render test asserting the Status chip appears for a row with
    a single-select value; a `WorktreeCardMeta` render test asserting the ↗ button
    calls `api.shell.openUrl` and the markup contains **no** `target="_blank"`
    (mirror `SourceControl.hosted-review-header-link.test.tsx`).
  - `cargo build -p agentum-desktop` compiles (no Rust change expected).
- **`qa.sh` asserts (browser QA gate — web surface):**
  - Bind a project's tracker, open its Tasks tab board → a card shows a visible
    **Status** chip.
  - Create an issue in the New Workspace wizard, create the workspace → the new
    sidebar row shows the issue `#N` **and** a non-null tracker/status chip.
  - Click the ↗ on the issue card → assert `api.shell.openUrl` fired with the
    issue's `html_url` (spy), since an external browser open can't be asserted
    in-app.

## Open questions

- **Tracker (needs a human decision):** file a **new** GitHub issue for this batch.
  #360 shipped in v0.78.0 and is **closed** (2026-07-17), and its ACs were "remove
  sidebar Board / establish per-project binding" — distinct from these follow-on
  fixes — so folding under it is not an option. Proposed new issue: title
  *"Per-project tracker fidelity: show Status on cards, keep the binding on
  wizard/local create, fix Open-on-GitHub"*, labels `type/feat` + `type/bug` +
  `area/desktop` + `priority/p2`. Set `tracker:` once filed (I can't create issues
  autonomously — run `/ship` or file it, then paste the URL).
- **"etc etc" (scope of enrichment):** A1–A2 propose Status + issue-type + updated
  time. Confirm whether the user also wants **milestone / reviewers / linked-PRs**
  now (full-stack, currently a non-goal) or later.
- **"Not assigned" symptom:** evidence says the issue `#N` *does* render and only
  the tracker/status chip is null (dropped bind). Confirm on the user's build that
  the number is visible and it's the status chip that's missing; if the number is
  also absent, a second bug in the `linkedIssue` path needs a separate look.
- **One-slice vs split:** this bundles one enhancement (A) + two bug-fixes (B, C)
  on one surface because the user asked for "all in one". OK to keep as one spec,
  or split B/C (bugs) from A (enhancement) into separate specs?
