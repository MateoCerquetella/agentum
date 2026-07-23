# Workflow

The end-to-end flow. Read top to bottom. Each step has a gate; each gate has a possible send-back.

---

## The picture

```
Stakeholder ask
      │
      ▼
┌──────────┐
│ Analyst  │  Socratic interview → spec draft   (skip if spec already concrete)
└─────┬────┘
      │  gate: Analyst (5 checks)
      ▼
┌──────────┐
│   PM     │  Refine AC, scope, priority
└─────┬────┘
      │  gate: PM
      ▼
┌──────────┐
│ Architect│  architecture.md: components, boundaries, tradeoffs, risks
└─────┬────┘
      │  gate: Architect
      ▼
┌──────────┐
│ Developer│  Implementation + tests + tasks.md update
└─────┬────┘
      │  gate: Developer
      ▼
┌──────────┐
│  Tester  │  Verify every AC; pass/fail with evidence
└─────┬────┘
      │  gate: Tester
      ▼
┌──────────┐
│ Reviewer │  Maintainability + completeness
└─────┬────┘
      │  gate: Reviewer
      ▼
    DONE
```

Send-backs are always **upstream** to the shallowest role that can fix the issue. See `ai/skills/validate_handoff.md` for the decision tree.

---

## Entry points

Two real entry points exist:

1. **`/sdd-spec-socratic`** — vague ask → drafted spec (BA-driven, runs the Analyst phase).
2. **`/sdd-spec`** — clear ask → drafted spec (developer-driven, skips Analyst).

After either, run **`/sdd-orchestrate`** to drive the spec through PM → Reviewer.

---

## Mode

- **HITL (default)** — orchestrator pauses at every gate.
- **Autonomous** — orchestrator decides at every gate, logs everything, hard-stops at 2 iterations per gate.

See `ai/orchestration/hitl_policy.md`.

---

## State

`ai/STATE.md` is the single state file. It holds: current spec, role, phase, mode, last 5 decisions, open send-backs. Read at the start of every orchestrate run; updated after every phase transition.

---

## Sizing guidance

- 3–8 specs per milestone is healthy for small/medium repos.
- One spec ≈ one screen. If it doesn't fit, split it before the Architect gate.
- A typical spec moves through the full loop in a few hours of human time (much less of clock time if autonomous mode is used for execute/test).

---

## What this workflow is NOT

- It is not BMAD — no 12+ personas, no Party Mode, no Enterprise track.
- It is not GSD — no 6 state files, no npm CLI, no installer.
- It is not a framework — it's a markdown scaffold you copy into your repo.
