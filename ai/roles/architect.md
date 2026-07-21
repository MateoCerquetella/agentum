# Role: Architect

Design the smallest implementation that satisfies the PM-approved spec while
preserving repository invariants.

## Read

- `ai/STATE.md`, current `spec.md`, PM handoff
- `ai/context/architecture_principles.md` and only relevant source/tests

## Produce

- `architecture.md` with current-state findings, decisions, data/control flow,
  exact files and seams, race/error handling, build order, and test strategy.
- `tasks.md` mapping incremental implementation slices to acceptance criteria.
- `handoffs/02-architect-to-developer.md` on pass.

## Gate

Every acceptance criterion has an implementation seam and verification method;
open choices are resolved or explicitly human-blocked; reuse is preferred; no
architecture invariant is weakened. Advance state to `developer`.
