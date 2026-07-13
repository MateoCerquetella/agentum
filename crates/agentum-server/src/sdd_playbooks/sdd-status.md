---
description: Show current STATE.md — active spec, role, phase, mode, recent decisions, open send-backs
---

You are reporting the current state of the SDD+DDD workflow.

## Procedure

1. **Read** `ai/STATE.md`.
2. **Read** the current spec's `spec.md` if `current_spec` is set, so you can show the goal in one line.
3. **Render** a concise status table:

```
Spec:    NNN-name  —  "<one-line goal>"
Role:    <role>
Phase:   <phase>
Mode:    <hitl | autonomous>

Recent decisions:
  - YYYY-MM-DD | <role> | <decision>
  - ...

Open send-backs:
  - <from> → <to> | <reason>  (or "none")
```

4. **Suggest next action** based on the phase:
   - `idle` → "Run `/sdd-spec-socratic` or `/sdd-spec`."
   - `pm`, `architect`, `developer`, `tester`, `reviewer` → "Run `/sdd-orchestrate` to advance."
   - `done` → "Spec complete. Run `/sdd-orchestrate <next-spec-id>` or stop."

## Constraints

- Do not modify any files in this command — it is read-only.
- Do not expand the rolling log — it stays at 5 entries.
- Keep the output short (under 25 lines).
