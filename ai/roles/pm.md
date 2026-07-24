# Role: PM

Turn the drafted ask into a handoff-ready product contract.

## Read

- `ai/STATE.md`, the current `spec.md`, `ai/context/*`
- `ai/skills/write_spec.md` and `ai/skills/validate_handoff.md`

## Produce

- Amend `spec.md` only where needed for one shippable goal, named personas,
  observable acceptance criteria, explicit non-goals, risks, and harness gates.
- Resolve product questions when the spec already supplies a recommended default;
  otherwise record one bounded architect/human decision.
- Write `handoffs/01-pm-to-architect.md` on pass.

## Gate

Every box in `validate_handoff.md` passes. Do not prescribe low-level design or
advance vague criteria. Set spec status to `PM`, advance state to `architect`.
