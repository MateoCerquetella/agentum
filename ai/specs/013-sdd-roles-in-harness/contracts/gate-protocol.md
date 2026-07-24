# Contracts — Gate protocol, phases & events (013)

The stable interfaces 013 introduces. Implementers and role-agent prompts must honor these.

> **Implementation note (decided during build):** the verdict is a **JSON file**,
> not a parsed chat line. This mirrors the existing QA gate (`qa/<id>.json` /
> `parse_qa_verdict`) — deterministic, house-aligned, and impossible to read a
> stray "PASS" from free-form prose. The `HARNESS_GATE:` line idea is dropped.

## 1. Gate verdict file

A role-agent writes its verdict to `.agentum-harness/roles/<phase>.json`:

```json
{ "passed": true,  "summary": "what passed" }
{ "passed": false, "summary": "the single most important gap" }
```

Parsing rules (`parse_role_verdict`):
- Valid JSON with `passed: bool` (+ optional `summary`) → `(passed, summary)`.
- **Absent, empty, or unparseable → the gate FAILS** (caller treats it as a
  CONCERNS/retry). Never a pass. (False-green guard — same rule as the QA gate.)

Each role must also persist its **artifact** before the verdict:
| Role      | Phase        | Artifact                                   |
| --------- | ------------ | ------------------------------------------ |
| PM        | Authoring    | refined `spec.md` (≥1 acceptance checkbox) |
| Architect | Architecture | `architecture.md`                          |
| Reviewer  | Review       | review notes (appended to `decisions.md`)  |

## 2. SpecPhase transition table

```
Authoring ──PASS──► Architecture ──PASS──► Decompose ──auto──► Executing ──all features done──► Review ──PASS──► Done
    │                    │                                          │                              │
 CONCERNS×N           CONCERNS×N                                (existing feature              CONCERNS×N
    ▼                    ▼                                      retry/block logic)                ▼
 Blocked              Blocked                                                                  Blocked

(Any Blocked state, when run config hitl_on_block=true, is instead AwaitingConfirm,
 resumable via POST /api/harness/{id}/confirm.)
```

- `Decompose` is agentless: `plan_from_spec` derives `feature_list.json` from `spec.md`
  checkboxes. Empty backlog → `Blocked("spec has no acceptance criteria")`.
- `Executing` is the **existing** feature loop; 013 does not alter its transitions.
- `×N` = up to `max_retries` (reused per-run knob).

## 3. New events (`/api/harness/events`)

Serde-tagged, snake_case (matching the existing `HarnessEvent` stream):

```jsonc
{ "type": "phase_changed", "harness_id": "...", "from": "authoring", "to": "architecture" }
{ "type": "gate_result",   "harness_id": "...", "role": "architect", "passed": true, "attempt": 1, "summary": "…" }
```

## 4. Run config additions (`feature_list.json` run knobs)

```jsonc
{
  "hitl_on_block": false,   // default: fully unattended, including QA. true → exhausted gate → AwaitingConfirm
  "max_retries": 2,         // reused for role gates
  "agent_tool": "claude",   // reused for role-agents (v1: one config for all phases)
  "agent_model": "..."      // reused for role-agents
}
```

## 5. Status additions (`GET /api/harness/{id}`)

`HarnessStatus` gains:
```jsonc
{ "phase": "Executing", "phase_attempts": 0 }
```
