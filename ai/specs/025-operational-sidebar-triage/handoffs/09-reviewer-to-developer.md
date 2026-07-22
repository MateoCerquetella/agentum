# Reviewer → Developer Send-back

## 1. Summary

Reviewer found two localized presentation-integration defects. Automated behavior is otherwise
accepted and no architecture or PM change is required.

## 2. Completed Work

- Reviewed spec completion, architecture consistency, maintainability, debt, tasks, and handoffs.
- Accepted Tester evidence and both prior correctness fixes.

## 3. Pending Work

- Preserve the three ordered zero-count operational headers when active filters match no rows.
- Make operational-rich density override legacy compact-card preference.
- Suppress the legacy inline-agent list for all operational rows.
- Add focused regressions for both paths.

## 4. Important Decisions

- Keep legacy empty-state and compact behavior unchanged for alternate grouping modes.
- This is a Developer send-back; the pure model and architecture remain sound.

## 5. Risks

- Without these fixes the visible hierarchy can contradict AC 1 and AC 4 despite green model tests.

## 6. Questions

- None.

## 7. Recommended Next Step

Developer should make the localized conditional/render fixes, add regression coverage, and
return for a narrow Tester retest followed by Reviewer sign-off.

## Failed gate evidence

- **Gate item:** all ACs pass per implementation review. **Failed:** AC 1 and AC 4 integration.
- **Evidence:** legacy `filtersHideAllRows` bypasses operational zero headers; legacy compact and
  inline-agent preferences override/duplicate operational-rich presentation.
- **Fix direction:** condition legacy behavior on non-operational grouping and make
  `operationalMeta` authoritative inside `WorktreeCard`.
