# SDD human-in-the-loop policy

The `mode` field in `ai/STATE.md` controls phase gates.

## `hitl`

- Pause after every role output.
- Present the evidence, failures, and recommended next transition.
- Advance only after the human approves or supplies a send-back decision.

## `auto`

- Advance routine gates when the written evidence satisfies the checklist.
- Log each transition and send-back in `ai/STATE.md`.
- Retry the same failed gate at most twice. A third failure changes the mode to
  `hitl` and stops with the exact unresolved decision.
- Stop immediately for missing authorization, destructive/release actions,
  credentials, an acceptance-criteria change, or a product choice whose options
  materially change user-visible behavior.

## Always human-gated

- Merging or releasing to shared branches/environments.
- Deleting durable user/project data.
- Expanding tracker/provider scope or weakening a verification gate.
- Choosing between incompatible product behaviors when the spec has no default.

`NEEDS-HUMAN` is a valid safe exit. It must name one concrete decision and the
evidence already gathered.
