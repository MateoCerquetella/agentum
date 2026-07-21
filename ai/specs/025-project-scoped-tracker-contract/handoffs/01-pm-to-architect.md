# Handoff — PM to Architect

- **Spec:** 025-project-scoped-tracker-contract
- **From:** PM
- **To:** Architect
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- A single product contract for canonical project-owned tracker configuration,
  shared by Settings, Tasks, workspace inheritance, and status automation.
- Ten observable acceptance criteria covering CRUD consistency, provider target
  isolation, migration, local/SSH routing, deletion isolation, and explicit
  defaults without runtime global fallback.

## Acceptance-criteria evidence

- **AC 1–2:** Define one typed `Repo.id`-keyed read/write/clear contract and
  cross-surface revision equality.
- **AC 3–4:** Define fail-closed task loading and provider-target fidelity.
- **AC 5–6:** Separate project configuration from immutable worktree ticket
  coordinates and remove ambiguous transition fallback.
- **AC 7–10:** Define deterministic migration, host isolation, cleanup isolation,
  and creation-time-only defaults.

## Verification

- `ai/skills/validate_handoff.md` — PASS (9/9 checklist items).
- `git diff --check -- ai/STATE.md` — PASS.

## Decisions and invariants

- `Repo.id` is the project ownership key; a repository slug is provider metadata,
  not UI/configuration identity.
- Project runtime resolution never consumes global last-used tracker state.
- Existing workspace/feature ticket URLs remain immutable execution targets.
- Settings and Tasks must use one atomic server writer, including over SSH.

## Remaining risks / next action

- Architect must choose a persistence/migration layout that preserves the
  existing GitHub status mapping and generic repo-field round-trip without
  creating a second durable writer.
