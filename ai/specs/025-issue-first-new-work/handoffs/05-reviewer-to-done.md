# Handoff — Reviewer to Done

- **Spec:** 025-issue-first-new-work
- **From:** Reviewer
- **To:** Done
- **Date:** 2026-07-22
- **Gate:** **SIGN-OFF**

## Final disposition

Reviewer signs off Spec 025 with **0 blockers and 0 should-fixes**. All eight
acceptance criteria pass, the approved architecture and invariants are intact,
and the prior keyboard-submit send-back is closed.

## Evidence

- Focused desktop UI: **106/106 PASS** across 6 files, including shared launch
  eligibility coverage for unavailable-agent, setup-blocked, and explicit
  compatible remote Manual outcomes.
- Vite production build: **PASS**, 7,239 modules transformed.
- Focused Harness server: **10/10 PASS**.
- Rust formatting and diff hygiene: **PASS**.
- `review.md`: final **SIGN-OFF**, with AC-by-AC, invariant, compatibility,
  security, scope, race, and prior-send-back disposition.

## Locked outcomes

- New Work owns deferred New/Existing issue intake and one final launch action.
- Eligible local GitHub work always receives an issue-derived spec.
- Autopilot owns the worktree only through confirmed `start-work` ownership;
  Manual converges the spec and opens one plain agent.
- Issue/worktree checkpoints make modal-lifetime Retry non-duplicating.
- Mouse and keyboard share one fail-closed eligibility predicate before any
  irreversible side effect.
- Legacy composer behavior and default `spec-from-issue` semantics remain
  compatible.

## Remaining human release gate

Installed-app `qa.sh` scenarios were not available in this worktree and remain
mandatory before release: scratch GitHub cardinality, both execution modes,
post-issue/post-worktree fault recovery, single owner/session, eligibility
matrix copy, and minimum-height layout. This handoff authorizes neither merge
nor release.

## Orchestrator action

Update Spec 025 status and `ai/STATE.md` phase to Done. Preserve the installed-
app QA deferral as an explicit release note/gate.
