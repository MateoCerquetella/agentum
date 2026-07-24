---
description: Produce a handoff contract for the current role transition (from→to) — used inside orchestration when a phase ends
---

You are producing a structured handoff contract to pass work from one role to another.

## Procedure

1. **Read** `ai/STATE.md` to identify the `current_role` (from) and infer the next role (to).
2. **Read** `ai/contracts/templates/handoff_contract.md` as the structure to fill.
3. **Read** the current spec folder `ai/specs/<current_spec>/` for the artifacts produced so far.
4. **Write** the handoff into `ai/specs/<current_spec>/handoffs/<NN>-<from>-to-<to>.md` (create the `handoffs/` subfolder if missing; `NN` is the next free 2-digit number).
5. Sections to fill (from the template): Summary, Completed Work, Pending Work, Important Decisions, Risks, Questions, Recommended Next Step.
6. Be **concise and explicit**. No hidden assumptions. If something is unclear, raise it as a Question rather than guess.
7. **Update `ai/STATE.md`**: append a decision line referencing this handoff.

## Send-back form

If this is a *send-back* (gate failed), include:
- which gate item failed
- the specific evidence (quote / line / file)
- the suggested fix direction (without prescribing implementation)

Route to the shallowest role that can fix it per `ai/skills/validate_handoff.md`.

## Anti-patterns

- ❌ Vague summary ("did some work, ready for next")
- ❌ Hiding unresolved questions inside "Notes"
- ❌ Recommending a next step that contradicts the gate outcome
