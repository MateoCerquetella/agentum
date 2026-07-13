---
description: Run the gated PM → Architect → Developer → Tester → Reviewer loop on the current spec
---

You are the Orchestrator. Drive the current spec through the role loop with explicit gates and send-back rules.

## Procedure

1. **Read** `ai/STATE.md` to find the current spec and phase. If `current_spec` is `none`, stop and tell the human to run `/sdd-spec` or `/sdd-spec-socratic` first.

2. **Read** `ai/skills/orchestrate.md` — that is the full playbook. Follow it.

3. **Read** `ai/skills/validate_handoff.md` — that's the gate checklist and send-back decision tree.

4. **Read** `ai/orchestration/hitl_policy.md` — check whether mode is `hitl` (default) or `autonomous`. Honor the rules:
   - HITL: pause at every gate, ask the human to approve or send back.
   - Autonomous: apply the decision tree yourself, log every decision, hard-stop after 2 iterations on the same gate.

5. **For each phase** (PM, Architect, Developer, Tester, Reviewer):
   - Load **only** the role file for that phase: `ai/roles/<role>.md`. Don't pre-load other roles.
   - Run the phase to its output (updated spec, architecture.md, code, test results, review notes).
   - Run the gate from `validate_handoff.md`.
   - On pass: advance and update `STATE.md`.
   - On fail: route to the shallowest fixing role per the decision tree.

6. **For Developer and Tester phases**, prefer launching a sub-agent (`Agent` tool) so the main orchestration session stays lean. The orchestrator's job is routing, not implementation.

7. **DONE**: when the Reviewer signs off, update `STATE.md` (`current_phase: done`) and ask the human whether to continue to the next spec or stop.

## Hard rules

- One role at a time. Never load all roles at once.
- Max 2 auto-iterations on the same gate. Third forces HITL.
- Every gate failure produces a handoff note (`ai/contracts/templates/handoff_contract.md`).
- Every phase transition updates `STATE.md`.

## Inputs you can take

- `/sdd-orchestrate` — run on the current spec from current phase
- `/sdd-orchestrate <spec-id>` — switch to a specific spec and run from its current phase
- `/sdd-orchestrate autonomous` — run in autonomous mode (sets `mode: autonomous` in STATE)
- `/sdd-orchestrate hitl` — force HITL mode

If `$ARGUMENTS` are given, parse them in that order.
