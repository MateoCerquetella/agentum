---
description: Initialize the SDD+DDD context for a new or empty project (walks through ai/context/* interactively)
---

You are kicking off the Empirical SDD+DDD starter for a new project. Your job is to help the human fill out `ai/context/` with real content — not template placeholders.

## Procedure

1. **Read** `ai/README.md` and `ai/context/*` so you understand the current state. If files contain only template placeholders (e.g., `<project-name>`, "Example: …"), treat the context as empty.

2. **Ask one focused question at a time** to fill out, in this order:
   - `project_vision.md` — problem, users, core features, constraints, success criteria
   - `personas.md` — name and goals of 1–3 real personas
   - `tech_stack.md` — what's already chosen, what's open
   - `architecture_principles.md` — keep the defaults unless the human pushes back
   - `domain_glossary.md` — 5–10 terms that matter in this domain
   - `current_milestone.md` — what's the immediate focus

   Don't ask everything at once. One section per turn unless the human is moving fast.

3. **Write each file** as soon as that section is complete — don't batch. The human should see progress.

4. **Initialize `ai/STATE.md`** with `current_spec: none`, `current_role: none`, `current_phase: idle`, `mode: hitl`.

5. **Finish** by telling the human:
   - "Context is set. You can now run `/sdd-spec-socratic` (BA-style, for vague ideas) or `/sdd-spec` (direct, for known asks) to draft the first spec."

## Constraints

- Don't invent answers. If the human is unsure, capture "TBD" and move on — they can fill it later.
- Keep each context file short. If a section grows past one screen, you're going too deep.
- Don't generate specs in this command — that's `/sdd-spec` or `/sdd-spec-socratic`.
