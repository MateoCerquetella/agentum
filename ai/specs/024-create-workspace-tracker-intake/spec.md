# Spec 024 — Create Workspace tracker intake

- **Number:** 024
- **Status:** Done            <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-desktop/ui/src/components/new-workspace`, `crates/agentum-server/src/routes`
- **Author:** Mateo (via Agentum SDD)
- **Date:** 2026-07-21

## Problem

When Mateo creates a workspace for the project he is currently working in, the
Tracker step can omit that project's issues, take too long to become useful,
and keep showing stale data. The flat, low-information issue list gives him no
status-based order or clear refresh feedback, and the adjacent “Draft with AI”
action gives him no control over which LLM writes the issue description.

## Goal

Deliver a trustworthy Create Workspace tracker intake for the selected project.

## Users / personas

- **Mateo (primary operator)** — opening Create Workspace for the repo/project
  he is about to work on; he needs to find the right current issue immediately,
  trust that the list is fresh, or draft a new issue with the LLM he chooses.
- **Multi-project agentum user** — switches between repositories whose GitHub
  Project bindings differ and must never see one project's issues under another.

## Acceptance criteria

1. **Selected-project fidelity.** On step 3, a selected git repo renders issues
   only from that repo's resolved Project binding. While the binding is loading,
   the UI renders a project-resolution loading state and does not fetch or paint
   issues from `settings.githubProjects.activeProject`; switching repos clears
   the previous repo's issue rows immediately. A selected repo with no binding
   renders a configure-tracker state instead of another project's issues.
2. **Current issues render.** Once the binding resolves, every pickable open
   issue returned by that Project view renders in the picker (with existing
   exclusion of PRs, draft items, redacted rows, and closed issues). The section
   identifies the resolved Project by title or `owner · project number`, so the
   operator can verify the source at a glance.
3. **Status-aware organization.** Each issue row renders its GitHub Project
   Status label/color when present. Rows are grouped or sorted in the selected
   view's configured Status-option order, preserve GitHub position within the
   same status, and place issues without a status in an explicit “No status”
   group last. If the selected view has no Status field, the picker preserves
   GitHub position without inventing statuses.
4. **Useful issue-picker UI.** The tracker area provides a text filter over issue
   number and title, a visible issue count, distinct loading/refreshing/empty/
   error states, and a retry/refresh button. Filtering and status organization
   are keyboard accessible, and selecting an issue continues to call the
   existing `onPickWorkItem` seam and visibly marks the linked row.
5. **Fast cached-first paint.** A fresh cached table paints synchronously. The
   wizard then performs a stale-while-revalidate refresh for the resolved
   Project without replacing visible cached rows with a spinner; concurrent
   requests remain deduplicated through `fetchProjectViewTable`. A successful
   refresh replaces the table, a failed background refresh retains the last
   good rows with a non-blocking stale/error indicator, and a manual refresh
   calls `fetchProjectViewTable(args, { force: true })`.
6. **Updates become visible.** Re-entering step 3, manually refreshing, or
   changing the selected repo/binding re-resolves the correct Project and can
   render newly added, removed, retitled, or status-changed issues without
   reopening Create Workspace. Late responses for a previously selected
   repo/Project are ignored.
7. **Drafting LLM is selectable in context.** The Create Issue panel renders a
   compact drafting-engine picker (Claude/Codex from the existing supported and
   detected Chat agents) beside “Draft with AI.” For Claude, it also renders the
   existing `CHAT_MODELS` choices; an engine without a model catalog clearly
   displays “default model” rather than fabricating choices. The current saved
   Chat agent/model preference initializes the controls, and a new choice is
   retained for the next draft/session using the existing Chat preference path.
8. **The selected LLM reaches generation.** Clicking Draft/Redraft sends the
   chosen `agent` and optional `model` through `draftGithubIssueBody` to
   `POST /api/github/issues/draft-body`; the server validates the agent with
   `resolve_chat_agent`, resolves the request model with
   `resolve_chat_model(request_model, resolved_agent)`, and calls the matching
   backend. Omitted fields retain today's config/default behavior, unknown
   agents/models return the existing actionable error path, and drafting never
   files the issue automatically.
9. **Tracker remains optional and non-blocking.** Missing bindings, unavailable
   trackers, slow refreshes, and AI draft failures render actionable inline
   states but never block creating the workspace or manually entering and filing
   an issue description.

## Scope & non-goals (YAGNI)

- **In:** The Create Workspace step-3 tracker resolver and presentation; pure
  issue status/filter/order derivation; cached-first background refresh and
  manual refresh; request-scoped AI agent/model controls for issue-description
  drafting; additive draft-body request wiring and tests.
- **Out:**
  - No new tracker provider and no redesign of the full Project Hub/Tasks board.
  - No polling loop, WebSocket subscription, or GitHub webhook system for the
    modal; updates arrive on entry/re-entry or explicit/background refresh.
  - No mutation of GitHub Project Status from the picker; status is read-only.
  - No change to issue filing semantics, gated-run eligibility, workspace launch,
    YOLO flags, tmux sessions, or the one launch path.
  - No arbitrary model discovery API. Use the existing Chat agent/model catalog;
    agents without a catalog use their server-configured default.
  - No automatic AI draft before the operator clicks Draft/Redraft, and no issue
    filing without the existing explicit Create action.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `TrackerSection` and `CreateIssuePanel`
  (`crates/agentum-desktop/ui/src/components/new-workspace/CreateWorkspaceWizard.tsx:1529,1751`)
  — extend the single step-3 tracker/create surface rather than add another modal.
- `deriveTrackerBindingTarget`, `deriveUnifiedTrackerStatus`
  (`crates/agentum-desktop/ui/src/components/new-workspace/create-workspace-wizard-model.ts:185,214`)
  — keep pure resolution/status logic testable; add an explicit unresolved state
  instead of borrowing the global Project during async binding reads.
- `GitHubProjectTable.rows`, `fieldValuesByFieldId`, `position`, and selected-view
  metadata (`crates/agentum-desktop/ui/src/shared/github-project-types.ts:157-204`)
  — the fetched table already carries the row values and GitHub position needed
  for status presentation and stable ordering.
- GitHub Project grouping/sorting primitives
  (`crates/agentum-desktop/ui/src/shared/github-project-group-sort.ts`) — reuse
  field-value and configured-option ordering behavior rather than write a second
  interpretation of Project Status.
- `fetchProjectViewTable` cache, in-flight dedupe, force-refresh semantics, and
  `getCachedProjectViewTable`
  (`crates/agentum-desktop/ui/src/store/slices/github.ts:137-181,1372-1406`) —
  build stale-while-revalidate UI behavior over these seams; do not add polling
  or a second cache.
- `deriveIssueOptions` / `onPickWorkItem`
  (`crates/agentum-desktop/ui/src/components/new-workspace/work-item-picker-model.ts:53-88`)
  — preserve pickability, dedupe, and workspace tracker persistence.
- `CHAT_AGENTS`, `CHAT_MODELS`, `pickChatAgent`, and the saved Chat preferences
  (`crates/agentum-desktop/ui/src/runtime/chat-client.ts:45-72`,
  `crates/agentum-desktop/ui/src/components/harness/ChatPage.tsx:70-82`) — share
  the supported engine/model vocabulary and persistence behavior.
- `draftGithubIssueBody` and shared chat backend dispatch
  (`crates/agentum-desktop/ui/src/runtime/github-issue-client.ts:197-245`,
  `crates/agentum-server/src/routes/chat.rs:2293-2324`) — extend the existing
  review-before-file endpoint; do not introduce a second LLM route.

### Build new

- A selected-repo binding resolution state that prevents global/stale Project
  fallback while a repo-specific binding is loading or absent.
- Pure tracker-list view-model helpers for status extraction, configured status
  order, “No status” fallback, stable within-status order, and text filtering.
- A compact status-aware issue picker header/list with project identity, count,
  search, cached/refresh state, and explicit retry/force-refresh controls.
- A small shared Chat draft preference seam so Create Workspace and Chat can
  initialize/persist the same supported agent/model choice without duplicating
  magic storage keys.
- Additive `agent` + `model` fields through the create-issue seam, TypeScript
  client payload, Rust `DraftBodyRequest`, and `chat::draft_issue_body` model
  resolution.

## Risks & invariants

- **Cross-project leakage:** the most important invariant is that rows already
  loaded for repo A never render after repo B is selected. Key async state and
  response acceptance by the full resolved Project identity and invalidate it
  immediately on repo/binding changes.
- **Refresh races:** a forced refresh or repo switch can overlap an older fetch;
  only the latest matching Project response may update the visible table.
- **Latency regression:** cached-first rendering must not create unbounded `gh`
  subprocesses. Reuse the store's concurrency gate and in-flight dedupe.
- **Status fidelity:** use selected-view field metadata/option order and existing
  Project sort helpers; never infer workflow order from status names.
- **Credential/model mismatch:** the UI must only offer supported engines/models,
  while the server remains authoritative and returns actionable errors. Missing
  credentials must leave manual editing and workspace creation available.
- **Backwards compatibility:** omitted `agent`/`model` on the draft-body request
  must preserve `chat.toml` and built-in defaults for older callers.
- **Architecture invariants:** this slice does not spawn agents, change YOLO
  flags, add polling, or bypass `spawn_agent_into_pane`; the one launch path and
  push-based session streaming remain untouched.

## Harness wiring (the gate)

- **feature_list.json entries:**
  1. `current-project-tracker-resolution` — selected-repo binding is authoritative;
     stale/global rows cannot leak across repo switches.
  2. `status-aware-fast-issue-picker` — status grouping/order, filter/count,
     cached-first background refresh, manual force refresh, and race-safe updates.
  3. `selectable-issue-drafting-llm` — shared preference control and additive
     client/server agent+model request wiring.
- **`verify.sh` asserts:** focused Vitest coverage for repo/binding transition
  races, issue eligibility, status order and “No status,” filtering, cached-first
  refresh state, force-refresh invocation, and LLM control defaults/persistence;
  Rust route/helper tests prove omitted/default and explicit agent/model
  resolution; `npm run build --prefix crates/agentum-desktop/ui`; relevant
  `cargo test -p agentum-server --lib` target(s).
- **`qa.sh` asserts:** in a real desktop browser, bind two repos to different
  Projects, switch between them, and capture that only the selected Project's
  status-grouped issues render; confirm cached rows paint while refresh runs,
  manual refresh reveals a changed issue/status, search filters the list, and
  selecting two different available drafting engines/models produces successful
  editable descriptions without filing until Create is clicked.

## Resolved architecture choices

- Keep the existing Chat local-storage model preference and extract one shared
  owner; do not add a settings migration or a second key.
- Prefer selected-view references to one single-select field named `Status`
  (grouping before sorting before visible fields), then one unambiguous table
  field match; otherwise preserve Project position without invented statuses.
