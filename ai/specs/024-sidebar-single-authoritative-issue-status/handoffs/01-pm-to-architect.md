# Handoff — PM to Architect

- **Spec:** 024-sidebar-single-authoritative-issue-status
- **From:** PM
- **To:** Architect
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- A one-slice product contract for removing duplicate tracker-owned lifecycle
  badges from the linked GitHub issue hover.
- The suppression boundary is explicit: only the six canonical Agentum labels,
  and only while a non-empty GitHub Project Status is resolved.
- Human `status/qa*` labels, ordinary labels, and the unbound label-only fallback
  remain visible.

## Acceptance-criteria evidence

- **AC 1-3:** `spec.md` defines the bound-project suppression rule, the exact
  preserved-label set, and the no-Project-status fallback.
- **AC 4:** `spec.md` provides a concrete render fixture and observable expected
  output for the focused Vitest regression.
- **AC 5:** scope and invariants constrain the change to presentation; tracker
  writes, caches, events, and metadata are explicit non-goals.
- **AC 6:** the gate names the focused sidebar Vitest suite and production Vite
  build commands.

## Verification

- `ai/skills/validate_handoff.md` — PASS (9/9 PM checks).
- `git diff --check` — PASS.

## Decisions and invariants

- GitHub Project Status is the sole lifecycle chip when available.
- Match only the exact canonical Agentum label names; do not hide arbitrary
  `status/` labels or human QA/release labels.
- Preserve all issue labels when Project status is absent so unbound repositories
  retain their only lifecycle signal.
- Reuse `WorktreeCardDetailsHover`, `useIssueProjectStatus`, and the existing
  static-render test harness; add no request, poll, event, or write path.

## Remaining risks / next action

- Architect should pin the helper location and test matrix while keeping the
  server-owned canonical-name list and UI mirror from drifting silently.
