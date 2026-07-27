---
schema: 1
id: SPC-1V8A31GHJAR2FNWWJAGB4S01EH
revision: 1
title: Spec: Chat-to-Features Pipeline (PARENT)
source: legacy-import:ai/specs/011-chat-to-features/spec.md@sha256:5a527287d7cb3dc87fa5feab07c192c1fb53e10e491b7ba3492dfff54fea61c3
---

# Spec: Chat-to-Features Pipeline (PARENT)

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

> # Spec: Chat-to-Features Pipeline (PARENT)
>
> > STATUS: PARENT — SPLIT into 011a–011d (see Notes).
> > Progress (worktree `feat/011-chat-to-features`, commits 6e1601b, 46bbbaf, NOT merged):
> > **011a DONE** (TaskSink seam + BoardSink + harness backlog writer + `POST /api/board/goals/{id}/harness-plan`) ·
> > **011b DONE** (GitHub Issues sink + agnostic provider selection) ·
> > **011c DONE** (Linear sink — desktop-creds token + sole-team) ·
> > **011d deferred**. Backend complete: 258 server-lib tests pass, clippy -D clean.
> > Pending: desktop UI trigger + merge (human/frontend-env gated). See `notes.md`.
>
> ## Goal
>
> A user describes work in plain language in the planner "New Goal" entry, and
> agentum turns it into tracked features in whatever task manager they have
> configured (GitHub, Linear, or the built-in board) and queues them for the
> harness to run.
>
> ---
>
> ## User Value
>
> Today the planner decomposes a goal into internal board cards, and the
> harness runs features from a hand-edited `feature_list.json` — two disconnected
> steps with no tie to the user's real issue tracker. This closes the loop:
> one natural-language description becomes real issues in the user's tracker
> *and* a ready-to-run harness backlog, without leaving agentum and without
> locking the user to a single provider.
>
> ---
>
> ## Requirements
>
> - Extend the existing planner goal flow (`POST /api/board/goals`) so a
>   decomposed goal also emits **features** to a configured task destination.
> - Introduce a **provider-agnostic task-sink seam**: one interface, multiple
>   backends (internal board, GitHub Issues, Linear), selected via the existing
>   `/api/preflight/check` detection. Internal board is the fallback when no
>   external manager is configured.
> - The **internal kanban board always mirrors** the created features as the
>   local view, regardless of where the source-of-truth lives.
> - Auto-write the created features into `.agentum-harness/feature_list.json`
>   via the existing bridge (`plan_from_spec` / `agentum_harness_board` /
>   `agentum_harness_plan`) — but **do not auto-run**. The harness runs only
>   when the user reviews the board and clicks **Run** (human-gated).
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] Submitting a natural-language goal creates N features in the
>       configured destination (board, GitHub, or Linear) and returns their
>       ids/urls.
> - [ ] When GitHub is configured, features are created as GitHub issues
>       (`gh issue create`); when Linear is connected, as Linear issues; when
>       neither is configured, as internal board items.
> - [ ] The provider is chosen automatically from `/api/preflight/check`; the
>       caller does not name a provider.
> - [ ] Every created feature appears on the internal kanban board as a card.
> - [ ] After creation, `.agentum-harness/feature_list.json` contains one
>       entry per created feature, with the harness in an **Idle** (not
>       running) state until the user clicks Run.
> - [ ] If no destination is reachable, the flow fails loudly with an
>       actionable message — it never silently drops features.
>
> ---
>
> ## Dependencies
>
> - **010 / 010a** — the `.agentum-harness/` surface + `feature_list.json`
>   schema and the `plan_from_spec` / `agentum_harness_board` bridge this spec
>   writes into.
> - Existing planner (`planner.rs`, `planner_prompt.md`, `POST /api/board/goals`).
> - Existing board primitive (`board_items`, `/api/board`, `WorkspaceKanbanDrawer.tsx`).
> - Existing integration code to unstub: `gh_create_issue` (gh.rs, currently
>   `not_available`), `linear_create_issue` (linear.rs, currently returns `None`;
>   frontend `linearCreateIssue` + token store already exist).
> - Existing `/api/preflight/check` provider detection.
>
> ---
>
> ## Risks
>
> - **Source-of-truth split-brain.** External manager is truth when configured,
>   board is truth otherwise — keeping the board mirror consistent with the
>   external tracker (and the harness backlog) is the central correctness risk.
>   Mitigation: one-way create in v1 (chat → sink → board mirror → harness);
>   defer bidirectional status sync to a later slice.
> - **Provider auth drift.** `gh`/Linear creation can fail at runtime (expired
>   token, missing scope). Must surface per-feature failures, not swallow them.
> - **Partial-failure on batch create.** Creating 5 features where #3 fails —
>   must report which landed and which didn't; no silent partial state.
> - **Scope creep.** This is a multi-surface feature; without a hard split it
>   becomes a giant spec (violates "small incremental delivery").
>
> ---
>
> ## Notes
>
> ### Out of scope (v1)
>
> - GitLab / Bitbucket / Azure / Gitea sinks (stubs today; defer).
> - Bidirectional sync (issue closed externally → board/harness updates).
> - A new dedicated chat panel — v1 reuses the existing planner goal entry.
> - Auto-running the harness — Run stays a human action.
>
> ### Proposed SPLIT (dependency-ordered vertical slices)
>
> - **011a — Internal-only vertical slice (FOUNDATIONAL).** The full pipeline
>   working end-to-end with zero external setup: define the provider-agnostic
>   `TaskSink` seam, implement the **internal-board sink**, extend the planner
>   goal flow to emit features through the sink, and auto-write
>   `feature_list.json` (harness Idle, human-gated Run). Proves the whole loop;
>   fits one screen.
> - **011b — GitHub Issues sink.** Unstub `gh_create_issue` via `gh issue
>   create`; wire it behind the seam; selected when preflight reports `gh`
>   configured. Read path already works.
> - **011c — Linear sink.** Unstub the Rust `linear_create_issue` GraphQL
>   mutation (frontend, token store, and UI already call it); wire behind the
>   seam; selected when a Linear workspace is connected.
> - **011d (deferred) — Sync & breadth.** Bidirectional status mirroring
>   (external ↔ board ↔ harness state) and additional providers (GitLab).
>
> Build order: 011a → (011b ∥ 011c) → 011d. 011a is the only slice that must
> land before any external provider is useful.
