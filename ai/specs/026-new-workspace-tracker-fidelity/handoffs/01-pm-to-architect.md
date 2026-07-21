# Handoff — PM to Architect

- **Spec:** 026-new-workspace-tracker-fidelity
- **From:** PM
- **To:** Architect
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- A focused product contract that makes the selected `Repo.id` and its
  server-resolved origin a closed tracker scope in New Workspace step 3.
- Eight observable criteria covering binding identity, honest unconfigured
  state, safe migration repair, repo-switch races, mixed-repository Projects,
  isolated configuration writes, SSH routing, and exact worktree linkage.

## Acceptance-criteria evidence

- **AC 1–3:** Bindings must match both project ID and resolved slug; migrated
  mismatches repair, explicitly configured mismatches remain user-owned and fail
  closed.
- **AC 4–5:** Old responses and cross-repository Project rows are ineligible.
- **AC 6–8:** Writes, SSH resolution, and selected-ticket persistence remain
  scoped to the selected project while unlinked workspace creation stays valid.

## Verification

- `ai/skills/validate_handoff.md` — PASS (9/9 checklist items).
- `git diff --check` — PASS.

## Decisions and invariants

- No global tracker fallback is allowed after a git project is selected.
- GitHub Project membership alone does not make another repository's issue
  eligible in this project-scoped picker.
- Automatic repair may replace only migrated configuration; explicit user
  configuration is preserved and surfaced for correction.
- The existing worktree creation and agent launch paths remain unchanged.

## Remaining risks / next action

- Architect must pin the canonical repository-slug comparison and row-filtering
  seam without duplicating provider fetches or weakening SSH host isolation.
