# Handoff — PM to Architect

- **Spec:** 025-issue-first-new-work
- **From:** PM
- **To:** Architect
- **Date:** 2026-07-22
- **Gate:** PASS

## Delivered

- `ai/specs/025-issue-first-new-work/spec.md` — one issue-first New Work launch
  contract with eight observable acceptance criteria and five locked product
  decisions.

## Acceptance-criteria evidence

- **AC 1–2:** The contract separates staged New issue intent from Existing issue
  selection and gives each a testable contextual final action.
- **AC 3:** The execution choice, eligible default, phase copy, and prohibited
  internal terminology are explicit rendering assertions.
- **AC 4:** One invariant requires an issue-derived `.agentum-harness` spec for
  both execution outcomes and removes spec scaffolding as a user decision.
- **AC 5–6:** Automatic and manual ownership map to existing `start-work`,
  `spec-from-issue`, and workspace activation seams while preserving exactly one
  agent driver.
- **AC 7:** Ordered progress, retained partial results, retry position, duplicate
  prevention, and surviving-worktree visibility are observable failure cases.
- **AC 8:** Unsupported inputs must render their reason before submission and
  cannot silently degrade from Autopilot to manual execution.

## Verification

- `ai/skills/validate_handoff.md` checklist — PASS (9/9).
- `git diff --check` — PASS.

## Decisions and invariants

- The issue is user-facing source; the Harness spec is always generated.
- SDD Autopilot defaults on only when eligible; Open manually stays explicit.
- Issue creation occurs only at the final action and its returned identity makes
  later in-wizard steps retry-safe.
- One launch path, fail-closed gates, keep-existing specs, durable worktrees,
  canonical tracker metadata, and honest eligibility remain protected.

## Remaining risks / next action

- Architect must define the smallest state-machine boundary that coordinates the
  existing issue-create, worktree-create, spec, and start-work seams without
  duplicating `useComposerState` or creating a second agent-launch path.
- The server-owned orchestration playbook references repo-side role contracts
  removed from HEAD. This handoff used their exact latest historical contents
  from commit `c98d2fa7`; align the server playbook and shipped scaffold in a
  separate maintenance fix rather than expanding this product slice.
