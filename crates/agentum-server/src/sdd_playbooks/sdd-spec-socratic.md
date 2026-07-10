---
description: Draft a spec via a Socratic five-pass interview (best for vague or stakeholder-driven asks)
---

You are running the Socratic spec-authoring flow, primarily for a Business Analyst or anyone with a fuzzy idea that needs crystallizing.

## Procedure

1. **Read** `ai/skills/write_spec_socratic.md` end-to-end. Follow it literally — it is the playbook.

2. **Read** `ai/context/project_vision.md` and `ai/context/personas.md` so you can ground the interview. (Don't load the entire `/ai` tree.)

3. **Run the five-pass interview** as defined in the skill:
   - Opening (one-sentence problem)
   - Pass 1: WHO
   - Pass 2: WHAT
   - Pass 3: WHY
   - Pass 4: WHEN IS IT DONE (acceptance criteria)
   - Pass 5: WHAT COULD GO WRONG

   One pass per turn. One or two questions at a time. Reflect each answer back before advancing.

4. **Crystallize** the answers into `ai/specs/<NNN>-<name>/spec.md` using `ai/specs/_template/spec.md` as the structure. Pick the next free `NNN` (3-digit number).

5. **Run the Analyst gate** (checklist in `ai/skills/validate_handoff.md`). Iterate once on any failed item.

6. **Update `ai/STATE.md`**: set `current_spec`, advance phase to `pm`, append a decision line.

7. **Hand off** to the PM. In HITL mode, ask the human: "Spec drafted. Approve handoff to PM?" before invoking `ai/roles/pm.md`.

## Anti-patterns

- Don't dump all five passes in one message.
- Don't accept solution-shaped answers ("I want a button…") — redirect to the problem.
- Don't proceed past a pass without a concrete answer.
- Don't load roles you don't need yet.
