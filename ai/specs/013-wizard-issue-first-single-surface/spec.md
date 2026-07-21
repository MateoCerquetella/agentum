# Spec 013 — Workspace wizard: honest tracker + create-issue-from-intent + single front door

- **Number:** 013
- **Status:** Draft             <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-desktop/ui` (New Workspace wizard) + `crates/agentum-server` (issue draft/create + wiki grounding)
- **Author:** Mateo (via `/sdd-spec`)
- **Date:** 2026-07-08

## Problem

Three things are wrong or missing in the "New workspace" wizard's step 3
("Agent & tracker"), all felt by the operator who's about to start work:

1. The **Tracker** section can read "No tracker — link one later" while the
   **Work item** section directly below it lists real issues (#266, #268 …) from
   a connected Project. The two sections read from **different** detection
   sources, so the wizard contradicts itself and looks broken.
2. The "Change / Configure tracker" control sits at the **bottom** of the step
   (inside the lower Work-item header), far from the tracker status it changes.
3. There's **no way to create an issue** for the thing you're about to build from
   inside this window — you must leave, file it elsewhere, and come back.
   Meanwhile the wizard still carries a dead-end "Start from a goal" step and a
   second, competing "Create Worktree" composer card (Image #3), so there are two
   create surfaces to confuse the operator.

## Goal

Make the New Workspace wizard the single, honest, issue-first create surface: one
tracker section that never lies, with its control on top; an in-window "create an
issue from what you want to do" flow grounded in the repo's Wiki + codebase; and
the legacy goal step + composer card removed, their unique powers migrated into
the wizard.

## Users / personas

The engineer who opens **New workspace** to begin a piece of work — who wants to
(a) see truthfully whether this repo has a tracker connected, (b) pick *or create*
the issue they're about to work, and (c) not be confronted with two different
create dialogs and a dead-end goal box.

## Acceptance criteria

**F1 — honest unified tracker**

1. Step-3's Tracker + Work-item content renders as **one** section whose header
   carries the "Change tracker" / "Configure tracker" control at the **top** (not
   in a lower Work-item header).
2. The tracker status text is driven by the **same** resolved Project the picker
   lists from (`resolvePickerProject` = per-repo binding ∨ global `activeProject`):
   when a Project resolves and issues load, the section shows the tracker as
   connected; when none resolves, it shows the honest "no tracker (optional)"
   empty state.
3. There is **no code path** where the section shows "No tracker" while the picker
   simultaneously lists ≥1 issue (the exact contradiction in the screenshot).

**F2 — create issue from intent (GitHub)**

4. The unified tracker section offers a **"Create issue"** affordance: a short
   free-text "what do you want to do?" description → a drafted issue (title +
   SDD-shaped body) the operator reviews / edits before filing.
5. The draft body is grounded in **both** the repo codebase snapshot
   (`gather_repo_context`) **and** its Wiki (`retrieve_wiki`) — `draft_issue_body`
   threads wiki context, not repo-context only.
6. Filing calls `POST /api/github/issues` (`createGithubIssue`) and, on success,
   binds the new issue as the linked work item via `applyLinkedWorkItem`, so the
   created workspace persists its tracker coords on create.
7. Create-issue is **optional and non-fatal**: a missing credential / no-repo /
   draft failure surfaces an inline error and never blocks creating the workspace.

**F3 — create issue (Linear)**

8. When the resolved tracker is Linear, "Create issue" files into Linear
   (`linear.createIssue`) using the **same** provider-agnostic drafted body, and
   binds the created Linear issue as the linked work item the same way.

**F4 — single front door**

9. The "Start from a goal" affordance and the goal / provision / details phases
   are removed from `NewWorkspaceComposerModal`; opening `new-workspace-composer`
   always renders `CreateWorkspaceWizard`.
10. `NewWorkspaceComposerCard` / `QuickTabBody` (the "Create Worktree" card,
    Image #3) is removed, and its unique capabilities are migrated into the
    wizard: (a) the "Start gated run" toggle + its precondition set (spec 005/008),
    (b) a pinned `initialBaseBranch`, (c) create-from opinionated opens (previously
    routed via `initialComposerPhase === 'details'`).
11. Every existing opinionated open (Tasks-page gated-run hop with `startGatedRun`,
    `linkedWorkItem`, `prefilledName`, `initialBaseBranch`) reaches an equivalent
    create through the wizard with no lost capability — a gated run started from
    the wizard hits the **same** `start_work` precondition set as before.

## Scope & non-goals (YAGNI)

- **In:**
  - Step-3 tracker / work-item unification + control placement (F1).
  - In-wizard create-issue-from-intent for GitHub (F2) and Linear (F3),
    wiki+codebase grounded.
  - Removing the goal step + composer card; wizard as the single front door with
    migrated gated-run / base-branch / create-from (F4).
- **Out:**
  - No tracker providers beyond GitHub + Linear.
  - No change to the harness gated-run **engine** (spec 005/008) — F4 only
    re-homes the entry UI.
  - No change to the work-item **picker** model (`work-item-picker-model.ts`:
    `deriveIssueOptions`, `resolvePickerProject`, `buildBindPayload`) — F1 reuses
    it as the single source of truth.
  - No redesign of steps 1–2 (Host, Repo & worktree).
  - Not force-deleting `NewWorkspaceGoalStep` / `NewWorkspaceProvisionStep`
    component files — F4 removes only their **use** in this modal; delete the
    files only if no other referrer remains.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- **Work-item picker + Project resolution** — `work-item-picker-model.ts`
  (`resolvePickerProject`, `deriveIssueOptions`, `buildBindPayload`,
  `WorkItemOption`) and `WorkItemPicker` inside `CreateWorkspaceWizard.tsx`
  (`origin/develop` ~L1007). F1's single source of truth.
- **Bind seam** — `applyLinkedWorkItem` on `useComposerState`, already wired via
  the wizard's `onPickWorkItem` (`CreateWorkspaceWizard.tsx` ~L210–226,
  `buildBindPayload`). Persists tracker coords on create (spec 012).
- **Issue draft/create (server)** — `POST /api/github/issues/draft-body` →
  `chat::draft_issue_body` (`routes/github.rs:297`, `routes/chat.rs:1844`) and
  `POST /api/github/issues` → `create_issue` (`routes/github.rs:207`).
- **Issue clients (UI)** — `runtime/github-issue-client.ts` (`createGithubIssue`
  :115, draft-body client :196).
- **Grounding helpers** — `gather_repo_context` (`chat.rs:235`) + `retrieve_wiki`
  (`chat.rs:640`, `async fn retrieve_wiki(workdir, messages)`).
- **Linear create** — `runtime-linear-client.ts` `createIssue` / `linear.createIssue`.
- **Gated-run machinery** — `useComposerState`'s `createGateMode: 'full'|'quick'`
  (:153, default `full`), `enableIssueAutomation` (:152), `initialStartGatedRun`
  (:127), and outputs `canStartGatedRun` / `startGatedRun` / `onStartGatedRunChange`
  (:234–241); `initialStartGatedRunProp` (`lib/composer-modal-props.ts`); the
  toggle UI in `NewWorkspaceComposerCard.tsx:745`; precondition
  `lib/start-gated-run-precondition.ts`.
- **Modal routing today** — `NewWorkspaceComposerModal.tsx` `phase` machine +
  `initialComposerPhase` / `firstGoalStepBlocker` (`lib/workspace-goal-step.ts:169`).

### Build new

- **F1** — merge the Tracker + Work-item sections; move the configure control to
  the top; compute the status label from the `resolvePickerProject` result
  (demote `deriveWizardTracker`'s `remoteUrl` heuristic as the display source). A
  small pure helper `deriveUnifiedTrackerStatus({ resolved, options, status })`
  in `create-workspace-wizard-model.ts` (unit-testable).
- **F2** — a wizard-local "create issue from intent" sub-panel (description →
  draft → review/edit → file → bind), with a pure state model
  (`create-issue-intent-model.ts`). Thread `retrieve_wiki` into
  `draft_issue_body` server-side (grounds on wiki+codebase); adapt `retrieve_wiki`
  to accept a query string (the title/description) rather than a chat transcript.
- **F3** — a provider branch in the create-issue *file* step: Linear via
  `linear.createIssue`, reusing the same drafted body.
- **F4** — widen `CreateWorkspaceWizardData` to accept `startGatedRun` +
  `initialBaseBranch`; add the "Start gated run" toggle + precondition wiring to
  the wizard (flip `createGateMode`/`enableIssueAutomation`, pass
  `initialStartGatedRun`); delete the goal / provision / details branches +
  `QuickTabBody` / card usage from `NewWorkspaceComposerModal`.

## Risks & invariants

- **Serde-alias-free (spec 012, memory).** Binding a *created* issue must reuse
  the existing `applyLinkedWorkItem` → `buildBindPayload` shape — no new aliased
  `Worktree` / linked-work-item fields (a serde alias there is a known wipe hazard).
- **Gated-run preserved (spec 005/008).** F4 must not regress `start_work`: the
  wizard's "Start gated run" toggle must hit the **same** `useComposerState` gate
  mode + precondition (`start-gated-run-precondition.ts`) and the same two-step /
  issue-automation path the card used.
- **Card removal blast radius.** Every `openModal('new-workspace-composer', …)`
  caller (each `telemetrySource` site) must land somewhere equivalent. Enumerate
  callers and verify each opinionated field (`startGatedRun`, `linkedWorkItem`,
  `prefilledName`, `initialBaseBranch`) is honored by the wizard **before**
  deleting the card.
- **Wiki grounding stays best-effort.** `retrieve_wiki` is async + optional;
  threading it into `draft_issue_body` must remain non-fatal (a wiki miss still
  drafts from repo context) and must not wedge the draft.
- **Fail-loud, non-blocking (silent-failure invariant).** Create-issue errors
  (no-creds, `no_github_repo`, gh/Linear down) show inline and never block
  creating the workspace.
- **One source of truth (F1 honesty).** After F1 there must be **no** second
  detection path that can disagree with the picker — the status label and the
  issue list read from the same resolved Project.

## Harness wiring (the gate)

- **feature_list.json entries (ordered):**
  1. `F1` — unify tracker + work-item into one honest section, control on top.
  2. `F2` — create-issue-from-intent (GitHub) + wiki-grounded draft body + bind.
  3. `F3` — create-issue Linear provider branch.
  4. `F4` — single front door: remove goal step + composer card, migrate
     gated-run / base-branch / create-from into the wizard.
- **`verify.sh` asserts:**
  - Rust: `cargo test -p agentum-server --lib` green — new `draft_issue_body`
    tests (F2): instructions include a wiki block when wiki present; still drafts
    when wiki absent.
  - UI: `bun run build` (vite) + `bunx vitest run` on the pure models — F1
    `deriveUnifiedTrackerStatus` test; F2/F3 `create-issue-intent-model` state
    machine test (draft → review → file → bind, error branches); F4 modal-routing
    (wizard-only) + `CreateWorkspaceWizardData` opinionated-field honoring.
  - No bare `tsc` (shared/* uses vite aliases; jsdom-free pure-model tests only).
- **`qa.sh` asserts (browser QA, staging):** open New workspace → step 3 shows
  ONE tracker section with the control on top; a connected repo never shows "No
  tracker" while issues list; "Create issue" drafts from a typed description and
  files + binds; there is no "Start from a goal" link and no second "Create
  Worktree" card anywhere; a gated run still starts from the wizard.

## Open questions

- **Linear draft body (F3):** reuse the GitHub `draft-body` route (the body is
  provider-agnostic markdown) or add a Linear-specific route? *Default:* reuse the
  same drafted body; only the *create* call branches by provider. Confirm no
  GitHub-specific framing leaks into the body.
- **Provider ambiguity (F2/F3):** which provider does "Create issue" target when a
  repo has BOTH a GitHub Project bound AND Linear connected? *Default:* follow the
  resolved tracker's provider; if genuinely ambiguous, show a provider toggle in
  the create-issue panel.
- **Provision step orphan (F4):** removing the goal path orphans
  `NewWorkspaceProvisionStep` (spec 010 F3) from the UI. Delete the component, or
  keep it wired elsewhere? *Default:* remove from this modal; delete the file only
  if no other referrer.
- **Telemetry:** does removing the card change any `workspace_created.source`
  values analytics rely on? Verify the wizard preserves each `telemetrySource`.
