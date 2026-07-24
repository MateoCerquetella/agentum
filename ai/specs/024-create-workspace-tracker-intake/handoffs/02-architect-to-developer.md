# Handoff — Architect to Developer

- **Spec:** 024-create-workspace-tracker-intake
- **From:** Architect (autonomous SDD loop step 2)
- **To:** Developer
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- `architecture.md` pins repository-authoritative binding state, request-keyed
  race handling, cached-first forced revalidation, status-field semantics, the
  shared Chat preference owner, and additive LLM request wiring.
- `tasks.md` divides the implementation into four incremental slices and a final
  gate, with every slice mapped to acceptance criteria and tests.

## Acceptance-criteria evidence

- **AC 1–6:** exact UI/model/store seams are assigned for closed binding states,
  keyed responses, Project-metadata grouping, useful states, and revalidation.
- **AC 7–8:** existing Chat catalogs/preferences and draft-body route are reused;
  optional agent/model have an end-to-end implementation and verification seam.
- **AC 9:** cached/error and AI failure designs retain workspace/manual paths.

## Verification

- Architect role gate — PASS (9/9 ACs have a file seam and verification method).
- `git diff --check` — PASS before handoff.

## Decisions and invariants

- A selected git repository never falls back to global active Project state.
- Status is one unambiguous single-select field named Status, selected-view
  references win, and no match means position-only rendering.
- Reuse the existing store cache/dedupe and force semantics; add no polling.
- Keep `GlobalSettings.chatAgent` plus the existing `agentum.chat.model` key as
  the shared preference owners; add no settings migration or duplicate key.
- Server resolves the agent before the optional request model and remains
  authoritative; omitted fields preserve current behavior.

## Remaining risks / next action

- Implement Slice 1 first and keep each async state write guarded by the full
  current binding/Project key. Run focused tests after every slice.
