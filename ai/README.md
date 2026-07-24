# `ai/` — Spec-Driven Development scaffold

This tree is the home of agentum's **SDD/DDD lightweight** workflow. It replaces
the retired `docs/superpowers/` convention.

```
ai/
  STATE.md                    # live phase/spec tracker — read first (/sdd-status)
  context/                    # durable project context (read by every role)
    project_vision.md
    architecture_principles.md
    domain_glossary.md
  skills/                     # how-to checklists the SDD skills read
    write_spec.md
    validate_handoff.md
    orchestrate.md
  orchestration/
    hitl_policy.md             # auto vs HITL gates and stop conditions
  roles/                       # one phase brief loaded at a time
    pm.md
    architect.md
    developer.md
    tester.md
    reviewer.md
  contracts/templates/
    handoff_contract.md
  specs/
    _template/spec.md         # copy this to start a new spec
    <NNN>-<name>/spec.md       # one focused, incremental spec per slice
```

## Flow

1. `/sdd-spec` (known ask) or `/sdd-spec-socratic` (vague ask) drafts
   `specs/<NNN>-<name>/spec.md` from `_template/spec.md`.
2. PM gate (`skills/validate_handoff.md`) → architect → developer → tester/reviewer.
3. **Execution runs through the Harness Engine.** A spec becomes a
   `.harness/feature_list.json` backlog, driven one feature at a time behind the
   `verify.sh` (unit) + `qa.sh` (browser QA) green gate.
4. Every change is still tracked as a GitHub issue and lands as a PR
   (`develop → staging → main`). The authoritative architecture + workflow guide
   is the repo-root `CLAUDE.md`.
