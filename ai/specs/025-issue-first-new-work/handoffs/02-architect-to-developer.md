# Handoff — Architect to Developer

- **Spec:** 025-issue-first-new-work
- **From:** Architect
- **To:** Developer
- **Date:** 2026-07-22
- **Gate:** PASS

## Delivered

- `architecture.md` — current-state findings, eight implementation decisions,
  staged control flow, exact seams, retry/error rules, build order, AC mapping,
  and verification strategy.
- `tasks.md` — three dependent implementation slices: deferred issue intent,
  mandatory issue-derived spec, and explicit execution/recovery.

## Acceptance-criteria evidence

- **AC 1–2:** A source union, staged editor, returned confirmed issue summary,
  explicit submit override, and contextual CTA prevent early/duplicate filing.
- **AC 3–4:** An execution union and mode-owned post-worktree branch remove
  technical choices while guaranteeing a local-GitHub issue-derived spec.
- **AC 5:** Strict reuse of `start-work` and ownership-gated workspace opening
  preserve one Harness driver and the centralized spawn path.
- **AC 6:** Optional `plan:false, converge:true` preserves manual spec edits and
  lets the unchanged manual activation open exactly one plain agent.
- **AC 7:** Issue/worktree checkpoints, field locking, ordered stage state, and
  Retry skipping give every partial success an observable recovery path.
- **AC 8:** One discriminated eligibility derivation renders exact blockers and
  prohibits silent Autopilot fallback.

## Verification

- Architect AC-to-seam matrix — PASS (8/8).
- Architecture invariant review — PASS.
- `git diff --check` — PASS before handoff.

## Decisions and invariants

- Use a resumable UI saga over existing authoritative APIs; do not pretend the
  cross-system operations are one rollback-capable transaction.
- Add only opt-in converge semantics to the existing spec route; preserve its
  default never-overwrite contract.
- Explicit `submitQuick` options activate the new behavior; unoptioned legacy
  callers remain compatible.
- Checkpoint immediately after external/local durability boundaries and never
  compensate by deleting an issue or worktree.
- Autopilot is strict ownership; one launch path and fail-closed gates remain
  non-negotiable.

## Remaining risks / next action

- Implement F1 → F2 → F3 in order. Keep the pure coordinator dependency-injected
  so duplicate-call and failure-position behavior is mechanically testable.
- Do not broaden this work into Chat navigation, repo-native Loop, Linear
  Autopilot, SSH Harness execution, registry schema, or cross-reopen recovery.
