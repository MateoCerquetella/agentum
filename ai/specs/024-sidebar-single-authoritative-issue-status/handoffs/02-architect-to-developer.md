# Handoff — Architect to Developer

- **Spec:** 024-sidebar-single-authoritative-issue-status
- **From:** Architect
- **To:** Developer
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- `architecture.md` pins the complete change to `WorktreeCardMeta.tsx` and its
  existing static-render test.
- `tasks.md` defines one implementation slice, F1, covering AC 1-6.
- All product choices are resolved: exact six-name matching, conditional on a
  non-empty Project status, with QA/ordinary/unbound fallback preservation.

## Acceptance-criteria evidence

- **AC 1-3:** D1-D3 define the exact filter and conditional fallback semantics.
- **AC 4:** the existing hoisted status mock and static-render harness are the
  selected pure verification seam.
- **AC 5:** D4 and the exact file list exclude all tracker, server, cache, event,
  and metadata changes.
- **AC 6:** build order and test strategy pin the focused Vitest and Vite build.

## Verification

- Architecture AC-to-seam mapping — PASS (6/6).
- Architecture invariant review — PASS (no invariant touched or weakened).
- `git diff --check` — PASS before handoff.

## Decisions and invariants

- Keep the helper beside its only UI consumer and cite the Rust canonical-name
  source; do not create a cross-crate sharing mechanism for six wire names.
- Compare exact case-sensitive names only; never filter by `status/` prefix.
- Filter synchronously from the existing status value; add no state/effect.
- Do not modify `IssueProjectStatusChip`, `TrackerPhaseChip`, server tracker
  writes, or cache/event plumbing.

## Remaining risks / next action

- Implement F1 in the two named UI files, run both green gates, and record the
  exact results in the Developer → Tester handoff.
