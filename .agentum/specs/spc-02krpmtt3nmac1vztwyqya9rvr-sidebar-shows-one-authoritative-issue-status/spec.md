---
schema: 1
id: SPC-02KRPMTT3NMAC1VZTWYQYA9RVR
revision: 1
title: Sidebar shows one authoritative issue status
source: legacy-import:ai/specs/024-sidebar-single-authoritative-issue-status/spec.md@sha256:346a0c8751bc161f28a0d9136bdd930c4fdae6fdb1d7030848fd4615f5bb4c3b
---

# Sidebar shows one authoritative issue status

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

> # Spec 024 — Sidebar shows one authoritative issue status
>
> - **Number:** 024
> - **Status:** Done              <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `crates/agentum-desktop/ui/src/components/sidebar`
> - **Author:** Mateo (via Agentum MCP `sdd-spec`)
> - **Date:** 2026-07-21
> - **tracker:** https://github.com/MateoCerquetella/agentum/issues/402
>
> ## Problem
>
> The sidebar hover for an active, GitHub-linked workspace can show the issue's
> status twice: once as the authoritative GitHub Project Status and again as an
> Agentum-managed `status/*` GitHub label. The repeated lifecycle metadata makes
> the card noisy and leaves the user unsure which value is authoritative.
>
> At intake, issue #402 demonstrated the live shape: the issue was in the Project's
> **In progress** column while its labels included `status/blocked`, and the hover
> rendered both in the same badge row.
>
> ## Goal
>
> Render exactly one authoritative lifecycle status in a linked GitHub issue's
> sidebar hover while preserving non-lifecycle labels and the label-only fallback.
>
> ## Users / personas
>
> - **Mateo (multi-agent operator)** monitors an active issue from its workspace
>   card and needs to understand its current state at a glance, without parsing two
>   tracker-owned status badges in the hover.
> - **An engineer using an unbound repository** still needs the existing
>   `status/*` label when no GitHub Project Status can be resolved.
>
> ## Acceptance criteria
>
> 1. When `useIssueProjectStatus` resolves a non-empty GitHub Project Status, the
>    issue hover **renders exactly one lifecycle-status chip**, using
>    `IssueProjectStatusChip`; Agentum's canonical tracker labels
>    (`status/todo`, `status/in-progress`, `status/in-review`,
>    `status/ready-to-test`, `status/done`, and `status/blocked`) do not render as
>    additional badges.
> 2. In that same bound-project state, the hover **continues to render** ordinary
>    issue labels and the human release labels `status/qa`, `status/qa-pass`, and
>    `status/qa-fail`; the fix must not broadly hide every `status/` prefix.
> 3. When no GitHub Project Status resolves (unbound repo, issue absent from the
>    project, loading/error, or missing issue URL), the hover **continues to
>    render all fetched issue labels**, including canonical `status/*` labels,
>    because the label may be the only lifecycle signal.
> 4. The filtering decision is a pure, unit-tested helper or equivalently isolated
>    render decision. A regression test with Project Status `In progress` plus
>    `status/blocked`, `status/in-progress`, `status/qa`, and `area/desktop`
>    **renders** one `In progress`, omits the two canonical tracker labels, and
>    preserves `status/qa` plus `area/desktop`.
> 5. The change **does not mutate** GitHub labels, Project fields, tracker events,
>    caches, or worktree metadata; it is presentation-only and leaves the existing
>    Project-status fetch/reconcile path unchanged.
> 6. The targeted sidebar Vitest file passes and
>    `npm run build --prefix crates/agentum-desktop/ui` completes without errors.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** the GitHub issue badge row in `WorktreeCardDetailsHover`; a small
>   canonical tracker-label classifier/filter; focused render tests.
> - **Out:**
>   - Changing `task_sink.rs` tracker writes, blocked escalation, GitHub Project
>     mappings, or label cleanup retries.
>   - Hiding human QA/release labels or arbitrary user-created labels that happen
>     to start with `status/`.
>   - Changing Linear issue badges, worktree activity dots, attention state, or
>     the GitHub issue/Project-status caches.
>   - Removing labels from GitHub itself; this spec changes only what the sidebar
>     hover displays when an authoritative Project status is present.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `WorktreeCardDetailsHover` and its issue badge row
>   (`crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.tsx:210`,
>   `:255`, `:320-333`) already combine the Project Status with `issue.labels`;
>   this is the single rendering seam to adjust.
> - `useIssueProjectStatus` / `IssueProjectStatusChip`
>   (`components/sidebar/IssueProjectStatusChip.tsx:92-206`) already provide the
>   authoritative Project option, stale-while-revalidate cache, event
>   invalidation, and worktree reconciliation. Their fetch and cache contracts
>   remain byte-for-byte behaviorally unchanged.
> - The existing static render harness
>   (`components/sidebar/WorktreeCardMeta.test.tsx:30-159`) already stubs Project
>   status and asserts that only the external lifecycle value renders; extend it
>   with real label-collision coverage instead of adding a second test harness.
> - The canonical Agentum tracker-label set is defined by
>   `crates/agentum-server/src/task_sink.rs:348-375`; it explicitly excludes the
>   human `status/qa*` lifecycle. The UI classifier mirrors these six public wire
>   names only; it does not import server internals across the crate boundary.
> - Bound-project transition cleanup (`task_sink.rs:820-830`) already attempts to
>   remove redundant labels remotely. The UI fix is the defensive presentation
>   layer for stale cleanup, blocked escalation, or delayed issue-cache refresh;
>   it does not duplicate the write path.
>
> ### Build new
>
> - A small pure predicate/filter near `WorktreeCardMeta.tsx` (or a sidebar-local
>   utility) that recognizes exactly the six Agentum-managed canonical labels and
>   suppresses them only while `projectStatus.status` is non-empty.
> - Focused Vitest cases covering bound suppression, QA/ordinary-label
>   preservation, and unbound fallback behavior.
>
> ## Risks & invariants
>
> - **Do not erase the fallback.** On an unbound repo, a canonical status label is
>   the issue's only lifecycle signal; filtering is conditional on a resolved
>   Project status (AC 3).
> - **Do not conflate harness and human QA lifecycle.** `status/qa*` is explicitly
>   outside the six Agentum-managed labels and remains visible (AC 2).
> - **GitHub Project remains authoritative.** The sidebar must not reintroduce the
>   removed local `TrackerPhaseChip` or derive a substitute status from worktree
>   state.
> - **Presentation-only.** No new network request, event subscription, poll, or
>   tracker mutation is introduced; existing push/event and cache paths remain
>   intact.
> - **Case fidelity.** GitHub label names are compared using their canonical exact
>   names; user labels with different names are preserved rather than guessed to
>   be internal.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:**
>   - `F1-sidebar-single-authoritative-status` — conditionally filter the six
>     canonical Agentum status labels from the GitHub issue hover and add focused
>     regression coverage.
> - **`verify.sh` asserts:**
>   - Run the focused `WorktreeCardMeta.test.tsx` Vitest suite: bound Project
>     status + canonical labels renders one lifecycle chip; QA/ordinary labels
>     survive; unbound canonical labels survive.
>   - Run `npm run build --prefix crates/agentum-desktop/ui`.
> - **`qa.sh` asserts:**
>   - Open issue #402's active workspace hover with a live Project status and
>     `status/blocked`: screenshot evidence shows one authoritative Project status,
>     no redundant canonical status-label badge, and any non-lifecycle labels.
>   - Exercise an issue in a repo with no Project binding (or a fixture that
>     resolves no Project status): screenshot evidence shows its `status/*` label
>     still present.
>
> ## Open questions
>
> - None. The narrow behavior is fixed by the issue report and existing
>   architecture: GitHub Project Status is authoritative when available; canonical
>   tracker labels are the fallback otherwise; human QA labels remain distinct.
