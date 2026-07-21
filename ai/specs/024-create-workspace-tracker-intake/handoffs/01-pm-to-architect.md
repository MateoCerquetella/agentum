# Handoff — PM to Architect

- **Spec:** 024-create-workspace-tracker-intake
- **From:** PM (autonomous SDD loop step 1)
- **To:** Architect
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- PM-approved `spec.md` for one shippable Create Workspace tracker-intake slice.
- Nine observable acceptance criteria covering selected-project fidelity,
  status-aware issue presentation, cached-first refresh/update behavior,
  request-scoped LLM selection, compatibility, and non-blocking failure states.
- Explicit non-goals prevent polling, tracker mutation, provider expansion,
  launch-path changes, arbitrary model discovery, and automatic filing.

## Acceptance-criteria evidence

- **AC 1–2:** selected repo binding is authoritative and the resolved Project
  identity plus complete pickable open-issue set must render.
- **AC 3–4:** status order/color, stable position, unassigned fallback, search,
  count, accessible selection, and explicit operational states are observable UI.
- **AC 5–6:** cached-first paint, background/forced refresh, retained last-good
  rows, and stale-response rejection are testable state transitions.
- **AC 7–8:** supported agent/model controls initialize, persist, traverse the
  client/server wire, and preserve omitted-field defaults without auto-filing.
- **AC 9:** every tracker/LLM failure remains inline and workspace creation plus
  manual issue entry remain available.

## Verification

- `ai/skills/validate_handoff.md` — PASS (9/9 boxes).
- `git diff --check` — PASS before handoff.

## Decisions and invariants

- Selected-repo Project identity wins; global/stale Project fallback is not
  permitted once a git repo is selected.
- Status order must come from GitHub Project metadata, never status-name guesses.
- Reuse the Project cache/concurrency gate and chat backend; do not add polling,
  a second cache, a second LLM endpoint, or a launch-path special case.
- Omitted draft agent/model fields remain backward-compatible.

## Remaining risks / next action

- Architect must pin one shared draft-model preference owner (recommended:
  existing Chat local storage for zero migration) and the unambiguous Status
  field selection rule (recommended: field named `Status`, else preserve
  GitHub position).
- Architect must line-verify current source anchors because the worktree already
  contains the separately authorized SDD-loop MCP/scaffold edits.
