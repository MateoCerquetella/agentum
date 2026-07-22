# Tasks — Operational sidebar triage

## Task 1 — Preference and operational model

**Status:** complete

- [x] Add `operational` to runtime/persisted grouping types and Workspace Options.
- [x] Make it the fresh/absent default while preserving every explicit persisted grouping and
  mapping legacy `parent` to `host`.
- [x] Export the shared state-to-`WorktreeStatus` resolver.
- [x] Implement/test pure operational classification, search, labels, ordering, counts, age, and
  settled disclosure.
- **Acceptance criteria:** 1, 2, 3 (model), 5, 7.

## Task 2 — Controls and shared row presentation

**Status:** complete

- [x] Lift transient search state through `Sidebar`; render conditional operational controls.
- [x] Implement/test responsive project-chip packing and overflow with existing repo filter state.
- [x] Route the configured worktree-search action to the inline field only while V2 is visible.
- [x] Add rich/settled bodies inside `WorktreeCard`'s existing interactive boundary.
- [x] Extend standard rows/virtual estimates and integrate operational rows in `WorktreeList`.
- **Acceptance criteria:** 3, 4, 5, 6, 8.

## Task 3 — Integration gate

**Status:** complete; real-browser evidence deferred to human-gated staging QA

- [x] Add focused model/interaction-seam coverage for stable rendered order, disclosure,
  persistence, status precedence, search focus, and virtual sizing.
- [x] Preserve action parity, active/reveal
  styling, narrow truncation, themes, and keyboard semantics.
- [x] Run focused Vitest suites and the desktop UI production build.
- [ ] Exercise seeded Needs You/Working/Ready/Settled states in the real desktop browser at both
  supported width extremes and both themes; record screenshots and any QA deferrals.
- **Acceptance criteria:** 1–9.
