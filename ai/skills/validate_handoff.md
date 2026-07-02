# Skill: validate_handoff (PM gate)

A spec is handoff-ready only if every box is checked. If any fails, surface it and
iterate once before advancing the phase in `ai/STATE.md`.

## Gate

- [ ] **One slice.** The goal is a single shippable increment (no hidden "and").
- [ ] **Problem before solution.** The Problem section names a user-felt pain, not
      a feature.
- [ ] **Persona named.** At least one concrete user + the moment they feel it.
- [ ] **Acceptance criteria are testable.** Every criterion has an observable verb
      and could become a `verify.sh` / `qa.sh` assertion.
- [ ] **Non-goals stated.** The spec says what it will NOT do.
- [ ] **Grounded in code.** Reuse-vs-build cites real crates / routes / components.
- [ ] **Invariants respected.** No criterion forces a break of an architecture
      principle (one launch path, YOLO translation, push streaming, …).
- [ ] **Harness wiring present.** The spec maps to `feature_list.json` entries and
      a green-gate definition.
- [ ] **STATE updated.** `ai/STATE.md` points at this spec, phase advanced, a
      decision line appended.

## On pass

Hand off to the architect. In HITL mode, ask the human to approve the handoff
first.
