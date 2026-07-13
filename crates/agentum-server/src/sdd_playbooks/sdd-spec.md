---
description: Draft a spec directly from a known ask (faster than Socratic — use when requirements are already clear)
---

You are drafting a spec directly, without the full Socratic interview. Use this when the human already has a concrete ask in hand.

## Procedure

1. **Read** `ai/skills/write_spec.md` for the authoring checklist and `ai/specs/_template/spec.md` for the structure.

2. **Read** `ai/context/project_vision.md`, `ai/context/architecture_principles.md`, and `ai/context/domain_glossary.md`. Don't load the entire `/ai` tree.

3. **Ask the human for the ask** if they haven't already provided it (in one message — Goal, Users, Acceptance Criteria, Risks). If anything is missing or vague, ask one focused follow-up.

4. **Write** `ai/specs/<NNN>-<name>/spec.md` populated from the ask. Pick the next free `NNN`.

5. **Run the PM gate** (checklist in `ai/skills/validate_handoff.md`). If anything fails, surface it and iterate once.

6. **Update `ai/STATE.md`**: set `current_spec`, advance phase to `pm`, append a decision line.

7. **Hand off** to the PM. In HITL mode, ask the human: "Spec drafted. Approve handoff to PM?"

## When to switch to `/sdd-spec-socratic` instead

- The human can't answer "what's the problem in one sentence" without naming a solution
- Acceptance criteria are missing or use vague verbs
- No persona has been named

If you see two or more of these, stop and suggest the Socratic flow.
