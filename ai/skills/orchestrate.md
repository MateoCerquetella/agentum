# Skill: orchestrate

Drive the current spec through one role at a time. `ai/STATE.md` is the durable
cursor; role outputs in `ai/specs/<current_spec>/` are the evidence.

## Phase order

`pm → architect → developer → tester → reviewer → done`

Never load future role briefs early. At each step:

1. Read `ai/STATE.md`, the current spec, durable project context, and only
   `ai/roles/<current phase>.md`.
2. Produce that role's required artifact or implementation.
3. Apply the role gate plus `ai/skills/validate_handoff.md` where applicable.
4. On pass, write a handoff from
   `ai/contracts/templates/handoff_contract.md`, advance `ai/STATE.md`, and
   append a concise decision entry.
5. On failure, route to the shallowest role able to fix the evidence:
   - unclear user value, scope, or acceptance criteria → PM
   - unsafe/incomplete design or unresolved boundary → Architect
   - incorrect/missing implementation or unit gate → Developer
   - missing/incorrect verification evidence → Tester
   - review-only documentation defect → Reviewer

## Autonomy and iteration limits

Read `ai/orchestration/hitl_policy.md`. In `auto` mode, decide routine gates
from evidence and continue. The third failure at the same gate is a mandatory
human stop. Never weaken acceptance criteria or a green gate merely to advance.

## Tracker status

If `spec.md` contains `tracker: <url>`, call `agentum_report_status` after each
transition: PM/Architect=`todo`, Developer=`in_progress`,
Tester=`ready_to_test`, Reviewer=`in_review`, done=`done`. Tracker failures are
non-fatal and must be recorded, not retried indefinitely.

## Completion

Reviewer sign-off requires all acceptance criteria mapped to evidence, required
gates green, no unresolved blocker, and no architecture-invariant regression.
Then set the spec status and `ai/STATE.md` phase to `done`. Release, merge, and
external publication remain human-gated unless separately authorized.
