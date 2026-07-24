# Handoff — Architect to Developer

- **Spec:** 026-new-workspace-tracker-fidelity
- **From:** Architect
- **To:** Developer
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- `architecture.md` pins the selected repository's closed scope across the
  canonical binding adapter, resolved slug, Project cache, request guards, row
  eligibility, and exact ticket persistence.
- `tasks.md` divides implementation into backend binding fidelity and wizard
  closed-scope slices with focused gates.

## Acceptance-criteria evidence

- **AC 1–3, 6–7:** One existing server seam owns `Repo.id` + resolved-slug
  validation, migrated CAS repair, configured mismatch preservation, and local/
  SSH fail-closed behavior.
- **AC 2, 4–5:** One full UI scope key and mandatory slug filter prevent global,
  cross-repository, cached, and late-response leakage.
- **AC 8:** Existing repo-switch reset and `applyLinkedWorkItem` create path are
  retained and explicitly tested.

## Verification

- Architecture traceability — PASS (8/8 acceptance criteria have an edit seam
  and verification method).
- Architecture invariants — PASS (no unresolved product choice; no global or
  local-host fallback; configured data remains user-owned).
- `git diff --check` — PASS.

## Decisions and invariants

- The server-returned resolved slug, not client git heuristics, scopes issue
  eligibility.
- Table request acceptance includes repo target + slug + Project, because
  Project identity alone cannot distinguish two repositories sharing a board.
- Correctness is based on render-time keyed eligibility, not effect cleanup
  timing.
- No new endpoint, provider fetch path, or worktree linkage path is allowed.

## Remaining risks / next action

- Implement F1 before F2. Preserve unrelated dirty-worktree changes and treat
  the existing Spec 025/partial mismatch code as shared in-progress work rather
  than replacing it wholesale.
