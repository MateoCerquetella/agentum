---
schema: 1
id: SPC-0ZPNQZ1GDAZVCDBJHCWH7GGA2A
revision: 1
title: Chat: preview, edit & regenerate before filing
source: legacy-import:ai/specs/003-chat-issue-preview/spec.md@sha256:2429f48fe026e704a744792103a0f540ef2dc5b4873cf87b416b0187fa2c9056
---

# Chat: preview, edit & regenerate before filing

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec 003 — Chat: preview, edit & regenerate before filing
>
> - **Number:** 003
> - **Status:** Draft             <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `crates/agentum-server/src/routes/chat.rs` + `crates/agentum-server/src/task_sink.rs` (+ `linear.rs`) + `crates/agentum-desktop/ui/src` (Chat)
> - **Author:** Mateo (via /sdd-spec)
> - **Date:** 2026-07-01
>
> ## Problem
>
> When a Chat conversation converges, clicking **"Create issues"** files the issue
> immediately and irreversibly. Worse, the filed issue is produced by a *second,
> separate* extraction call — so what actually lands on GitHub/Linear can differ
> from the prose breakdown the chat just showed, and the user never sees the real
> title/body/priorities until the issue already exists. There is no way to look at
> what will be created, fix a wrong title or priority, regenerate it, or choose how
> it's filed. The result feels "too simple": a black-box one-shot with no control.
>
> ## Goal
>
> Before Chat files anything, show the user an **editable draft** of exactly what
> will be created — which they can regenerate, edit, and only then confirm — with
> control over the issue split (one issue vs. one-per-task), the provider, and
> labels.
>
> ## Users / personas
>
> **Mateo, driving SDD intake from the Chat page.** He has just talked a feature
> through with the interviewer and is about to file it as a GitHub issue for the
> agentum project. In that moment he wants to *see the exact title, summary, and
> priority-ordered tasks that will be filed*, tweak a task that's mis-prioritised
> or badly worded, decide whether it should be one epic-style issue or several,
> pick GitHub vs. Linear, add a label — then commit. Today he gets none of that:
> he clicks and hopes.
>
> ## Acceptance criteria
>
> 1. **Preview returns a draft, files nothing.** `POST /api/chat/issues/preview`
>    (new) runs the same extraction as `chat_issues` and **returns**
>    `{ title, summary, tasks: [{ title, detail, priority }], body }` — where `body`
>    is the `compose_issue_body` render — and makes **zero** tracker calls (no `gh`,
>    no Linear GraphQL). Verified by a unit test asserting the handler returns a plan
>    and no create path is invoked.
> 2. **The UI shows an editable draft before filing.** After the interview converges
>    and the user asks to create, the Chat surface renders the returned plan as an
>    **editable** panel: the feature title, the summary, and each task's
>    title / detail / priority are all editable; nothing is filed at this point.
> 3. **Regenerate replaces the draft.** A **Regenerate** action re-runs
>    `/api/chat/issues/preview` and replaces the shown draft with a fresh plan. If the
>    user has unsaved edits, regenerating is confirmed first (edits are discarded on a
>    fresh generate — the regenerated plan is authoritative).
> 4. **Split, provider, and labels are user-chosen on the draft.** The draft panel
>    lets the user pick: **split** = *one issue with a sub-task checklist* (default,
>    today's behaviour) **or** *one issue per task*; **provider** = GitHub (default)
>    or Linear; and **labels** (0+ free-text). These choices are sent with Confirm.
> 5. **Confirm files exactly the shown draft.** `POST /api/chat/issues` accepts an
>    optional client-supplied `plan` (+ `split`, `provider`, `labels`); when `plan`
>    is present the handler **skips extraction and files that plan verbatim**. The
>    created issue's title and body match the draft the user confirmed
>    (what-you-see-is-what-you-file). Verified by a unit test on the create-from-plan
>    path.
> 6. **Only Confirm creates.** Preview and Regenerate never file; a second Confirm
>    after success does not double-file (the panel is dismissed/locked on success).
> 7. **Labels reach the tracker.** `NewFeature` carries `labels: Vec<String>`
>    (default empty, so existing callers are unchanged); `gh issue create` adds one
>    `--label <l>` per label. Linear label application is either wired via labelIds or
>    an explicit, documented no-op for v1 (see Open questions) — never a silent drop
>    without a note.
> 8. **Errors surface at preview time.** No-LLM-credentials, `no_tasks` (nothing
>    extractable), unknown provider, and chosen-but-unconnected provider
>    (`no_github_repo` / `no_linear`) are returned as the existing typed envelopes on
>    **preview**, before any filing — so the user never reaches Confirm on a plan that
>    can't be filed.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:**
>   - A preview (extract-only) endpoint + a create-from-supplied-plan path.
>   - An editable draft panel in the Chat UI (title/summary/tasks/priority) with
>     Regenerate, Confirm, Cancel.
>   - Split choice (one-issue-checklist vs. one-issue-per-task), provider choice
>     (GitHub/Linear), and free-text labels, chosen on the draft.
>   - `NewFeature.labels` + GitHub `--label` wiring.
> - **Out:**
>   - The **board / Kanban / status-sync** work (real GitHub Project statuses as
>     Kanban lanes, drag-to-move write-back, and "projects-first" repo→project
>     mapping). That is the agreed roadmap for **spec 004+** — see "Roadmap" below.
>     This spec touches Chat issue *creation* only, not any board view.
>   - Editing the *interview* itself, streaming changes, or model selection for the
>     extraction call (it stays `DEFAULT_MODEL`).
>   - GitHub sub-issue / parent-child linking for the multi-issue split (v1 files N
>     independent issues; no epic linkage).
>   - Repo-label validation / autocomplete (labels are free-text v1).
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - **The whole extraction half** of `chat_issues` (`routes/chat.rs:950-1019`):
>   `gather_repo_context` (`chat.rs:207-271`) → `call_anthropic(DEFAULT_MODEL, …, 2048)`
>   (`chat.rs:999`) with `EXTRACT_INSTRUCTIONS` (`chat.rs:822`) + `EXTRACT_USER_PROMPT`
>   (`chat.rs:828`) → `extract_feature_plan` (`chat.rs:1197-1211`). Reuse verbatim;
>   the preview endpoint is this half minus the filing.
> - **`FeaturePlan` / `SubTask` / `Priority` / `parse_priority` / `compose_issue_body`**
>   (`chat.rs:852-942`) — the draft's data model and body renderer already exist. The
>   preview returns these; Confirm re-composes from the (edited) plan.
> - **Provider resolution + create paths**: `resolve_provider` + `IssueProvider`
>   (`chat.rs:1033-1043`), `create_github_issue` (`chat.rs:1050-1136`),
>   `create_linear_issue` (`chat.rs:1143-1191`). Confirm reuses these; `provider` is
>   **already** a `ChatIssuesRequest` field (`chat.rs:846-847`) — the UI just never
>   sends it today.
> - **`created[] / failed[]` response shape** (`chat.rs:1119-1133`) already supports N
>   results — the multi-issue split loops the existing create fn and fills the arrays;
>   the UI render is unchanged.
> - **`TaskSink` + argv builders** (`task_sink.rs:24-27` `NewFeature`; `gh_create_argv`
>   / `gh_create_argv_with_repo` `task_sink.rs:291-304`) — extend, don't replace.
> - **UI seam**: `ChatPage.tsx` `createIssues` (`~326-361`) + the "Create issues"
>   button (`~486-513`); client `createIssuesFromChat` + types `CreatedIssues` /
>   `CreatedIssue` (`chat-client.ts:212-292`). The draft panel slots between the
>   button and the create call.
>
> ### Build new
>
> - **Server — preview endpoint** `POST /api/chat/issues/preview`: refactor
>   `chat_issues` into `extract_plan(state, req) -> FeaturePlan` (shared) + the
>   existing filing; preview calls `extract_plan` and returns the plan + `body`.
> - **Server — create-from-plan**: extend `ChatIssuesRequest` with optional
>   `plan: Option<ClientPlan>`, `split: Option<"single"|"per_task">`,
>   `labels: Vec<String>`. When `plan` is present, skip extraction and file it. Absent
>   `plan` keeps today's extract-then-file (back-compat).
> - **Server — multi-issue split**: when `split == "per_task"`, file one issue per
>   task (title = task title, body = task detail + priority) via the existing create
>   fn in a loop; else today's single-issue-with-checklist.
> - **Server — labels**: add `labels: Vec<String>` to `NewFeature`
>   (`task_sink.rs:24-27`, default empty) and emit `--label` per label in the `gh`
>   argv; thread `labels` from the confirm request. Linear labels per Open questions.
> - **UI — draft panel** in `ChatPage.tsx`: editable title/summary/tasks(+priority),
>   split toggle, provider selector, label input, and Regenerate / Confirm / Cancel.
> - **UI client**: `previewIssuesFromChat()` (POST `/preview`) and a `confirm` variant
>   of `createIssuesFromChat()` carrying the edited plan + split + provider + labels;
>   new `DraftPlan` type.
>
> ## Risks & invariants
>
> - **What-you-see-is-what-you-file (the whole point).** Confirm MUST file the
>   client-supplied plan, not re-extract. Re-extracting on confirm reintroduces the
>   exact drift this spec removes. Guard with a test that a confirmed edited title
>   lands verbatim.
> - **Chat rule: GitHub/Linear only, never the internal board.** Keep
>   `resolve_provider`'s closed set (`chat.rs:1033-1043`); a supplied `plan` must not
>   open a board path.
> - **OAuth invariants on the preview extraction call.** Preserve `build_system`'s
>   byte-exact Claude Code identity block for OAuth (`chat.rs:637-645`) and the
>   trailing `EXTRACT_USER_PROMPT` user turn (`chat.rs:984`) — Anthropic 401s / 400s
>   otherwise. The preview call is the same call, so keep it identical.
> - **`NewFeature` is cross-cutting.** It's used by `board_goals` and the harness too;
>   add `labels` as a defaulted field so those callers compile and behave unchanged.
> - **Multi-issue split contradicts a deliberate design choice.** `chat.rs:810-815`
>   states "One feature = one issue, not N flat issues." The split is *opt-in* and the
>   **default stays single-issue-with-checklist**, so we don't regress the intended
>   UX; the per-task mode is the escape hatch the user asked for.
> - **No double-file.** Preview/Regenerate are idempotent reads; only Confirm writes,
>   and the panel locks on success.
>
> ## Harness wiring (the gate)
>
> - **`feature_list.json` entries (ordered):**
>   1. `chat-plan-preview` — refactor `chat_issues` → `extract_plan` + `POST
>      /api/chat/issues/preview` returning `{title,summary,tasks[],body}`, files
>      nothing.
>   2. `chat-confirm-plan` — `chat_issues` accepts optional `plan` (+ `split`,
>      `provider`, `labels`); present → file verbatim, absent → today's behaviour.
>   3. `task-sink-labels` — `NewFeature.labels` + `gh --label` wiring (+ Linear per
>      Open questions).
>   4. `chat-draft-ui` — editable draft panel in `ChatPage.tsx` (edit, Regenerate,
>      split/provider/labels, Confirm/Cancel) + `previewIssuesFromChat` /
>      confirm client.
> - **`verify.sh` asserts (unit gate):** `cargo test -p agentum-server` — new tests:
>   (a) preview returns a plan and invokes no create path; (b) confirm-with-plan files
>   the supplied title/body verbatim (no re-extract); (c) `gh_create_argv` includes
>   `--label` for each label; (d) `resolve_provider` still rejects non-GitHub/Linear;
>   (e) `split=per_task` yields N `NewFeature`s. Plus `npm run build --prefix
>   crates/agentum-desktop/ui`.
> - **`qa.sh` asserts (browser QA gate):** drive the Chat page → converge a short
>   interview → click Preview → the editable draft renders with the extracted
>   title/tasks → edit a task title → Regenerate (draft refreshes) → set a label and
>   Confirm → the success card links a created issue whose title/body reflect the
>   edits. Screenshot each step.
>
> ## Open questions
>
> - **Endpoint shape:** confirm the recommended split — a *new* `POST
>   /api/chat/issues/preview` (extract-only) + reuse `POST /api/chat/issues` with an
>   optional `plan` for confirm (absent = back-compat) — vs. a dedicated `POST
>   /api/chat/issues/confirm`. (Recommendation: preview + overload the existing
>   create route; smallest surface.)
> - **Regenerate vs. edits:** confirm that Regenerate discards unsaved edits (with a
>   confirm prompt) rather than merging. (Recommended.)
> - **Multi-issue split linkage:** v1 files N independent issues with no parent/epic
>   linkage — acceptable, or is a checklist-in-a-parent-that-links-children wanted?
>   (Recommended: independent issues v1.)
> - **Linear labels:** GitHub `--label` is straightforward; Linear needs name→id
>   resolution. Ship GitHub labels v1 and defer Linear labels (documented no-op), or
>   wire Linear labelIds now? (Recommended: GitHub v1, Linear deferred.)
> - **Scope trim:** if the first ship must be smaller, increments 3 (labels) and the
>   `per_task` split can move to a 003b — the spine (preview + edit + regenerate +
>   confirm, single-issue GitHub) still delivers the core value. Confirm whether to
>   keep all four increments in 003.
>
> ## Roadmap (agreed, not this spec)
>
> The board asks map to follow-up specs (target end state chosen: projects-first):
>
> - **004 — Project Kanban (read):** render the existing GitHub Projects v2 `Status`
>   single-select (Todo/In Progress/QA/Done) as Kanban lanes instead of the hardcoded
>   2-column `Open`/`Done` issues board. The status data is *already fetched*
>   (`gh_projects.rs`); enable `BOARD_LAYOUT` + group-by-Status and reuse the generic
>   `TaskKanbanBoard`.
> - **005 — Two-way status sync:** implement the stubbed `updateProjectV2ItemFieldValue`
>   mutation so dragging a card between status lanes persists to the GitHub Project.
> - **006 — Projects-first:** make a GitHub Project the primary board organizer with a
>   persisted repo→project mapping (new state; the app has none today).
