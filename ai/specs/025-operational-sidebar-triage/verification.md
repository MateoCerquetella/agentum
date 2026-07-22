# Tester Verification

## Verdict

**FAIL — send back to Developer.** Two code-level acceptance failures are reproducible by
inspection and the current model test expectation. Browser-only evidence is separately deferred
because Playwright MCP is not connected in this session.

## Acceptance criteria

1. **PASS.** The pure model emits exactly Needs You, Active, and Settled headers with full
   filtered counts, and focused model tests pass.
2. **PASS.** The shared resolver supplies mutually exclusive urgent-precedence statuses; its
   focused status tests pass.
3. **PASS.** Inline query, project controls/overflow, existing add action, composed repo filters,
   and operational-only shortcut focus are implemented; focused model/overflow/focus tests pass.
4. **FAIL.** `WorktreeList` resolves aggregate status across panes but chooses `stateTimestamp`
   from the newest entry regardless of which entry produced the winning status. Repro: an older
   awaiting-input pane plus a newer working pane renders `Needs input` with the working pane age.
5. **FAIL.** Settled uses the common comparator, which checks `isPinned` before activity time.
   Repro: a pinned older settled workspace sorts ahead of a newer unpinned settled workspace;
   the current model test encodes that incorrect expectation.
6. **PASS by boundary inspection.** Operational items still render through the existing
   `WorktreeCard` interaction owner and viewport callbacks; no alternate activation/context/
   drag/select/reveal implementation was added. Runtime browser exercise is deferred below.
7. **PASS.** Absent/invalid grouping normalizes to operational, explicit supported choices are
   preserved, legacy parent maps to host, and focused hydration tests pass.
8. **DEFERRED (environment).** Playwright MCP is not connected, so 220/500 px light/dark
   screenshots, keyboard walk, and contrast/runtime interaction evidence cannot be captured.
   No browser pass is claimed.
9. **PASS.** Six focused files pass 17/17 tests; production Vite build exits 0 after 7,242
   modules with only existing chunk/import warnings; `git diff --check` passes.

## Test evidence

- Focused Vitest: 6 files, 17 tests passed.
- Targeted grouping hydration: 1 passed, 71 skipped.
- Startup hydration: 3/3 passed.
- Full UI-slice suite: 68 passed and 4 pre-existing unrelated failures (legacy pet migration
  and three task-navigation history assertions).
- Production Vite build: exit 0.

## Required fixes

1. Sort Settled strictly by most recent activity, without pinned-first precedence, and update
   the focused expectation/regression test.
2. Derive the displayed state timestamp from the same winning status signal, or omit it when
   that association cannot be proven; add a mixed-pane precedence/age regression test.
