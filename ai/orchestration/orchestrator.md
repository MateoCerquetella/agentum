# Orchestrator

The Orchestrator is **a role you (or the AI) play**, not a separate program. It coordinates the spec through the role loop, runs the gates, and routes send-backs.

In Claude Code, the `/sdd-orchestrate` slash command puts you in this role.

---

## What the Orchestrator does

1. Reads `ai/STATE.md` to find the current spec and phase.
2. Reads `ai/skills/orchestrate.md` for the playbook.
3. Reads `ai/orchestration/hitl_policy.md` to pick up the mode.
4. Walks the current spec through PM → Architect → Developer → Tester → Reviewer.
5. Runs `validate_handoff.md` at every transition.
6. Updates `STATE.md` after every transition.
7. Loads role files **on demand** — never all at once.

---

## What the Orchestrator does NOT do

- Does not author specs (use `/sdd-spec` or `/sdd-spec-socratic`).
- Does not implement code itself in non-trivial cases — it spawns a sub-agent (Developer) and routes the result.
- Does not skip gates "because it's obvious."
- Does not load every role file every turn.

---

## Token economy

The Orchestrator's main session should stay lean:

- It holds `STATE.md`, `orchestrate.md`, `validate_handoff.md`, and the current spec — that's it.
- For Developer / Tester phases, it delegates to a sub-agent which loads the role file + spec + architecture in isolation.
- For PM / Architect / Reviewer phases, it loads only that role file for the current turn.

This is why a typical spec cycle reads 4–6 small files per phase — not the entire `/ai` tree.

---

## When the Orchestrator hands off to a human

- Any time HITL mode says to pause (every gate).
- Any time a role raises a `question_contract.md` — questions are always for humans.
- After 2 auto-iterations on the same gate (hard rule).
- When the spec is DONE — confirm next step.

---

## See also

- `ai/skills/orchestrate.md` — full playbook
- `ai/skills/validate_handoff.md` — gates and send-back tree
- `ai/orchestration/handoff_rules.md` — what every handoff must contain
- `ai/orchestration/hitl_policy.md` — HITL vs autonomous
- `ai/orchestration/context_policy.md` — context discipline
