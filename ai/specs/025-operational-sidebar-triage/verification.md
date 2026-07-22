# Tester Verification — Rework Retest

## Verdict

**FAIL — send back to Developer.** The Settled-ordering defect is fixed, but the state-age fix
still accepts stale pane entries and can associate their timestamp with a status that actually
came from watchdog, title, or retained-state fallback. Browser-only evidence remains deferred
because Playwright MCP is not connected; no screenshots or browser pass are claimed.

## Acceptance criteria

1. **PASS.** `buildOperationalSidebarRows` always emits Needs You, Active, and Settled in that
   order, with counts taken from the complete filtered buckets. Focused model coverage verifies
   fixed order, full counts, and exclusive membership.
2. **PASS.** Operational classification consumes `resolveWorktreeStatusFromState`, whose shared
   resolver enforces permission > working > done > active > inactive. Each worktree is inserted
   into exactly one model bucket.
3. **PASS.** The implementation contains inline search across all four specified fields, All and
   project controls with overflow, the existing add action, composed project filtering, and
   operational-only shortcut focus routing. The focused search/overflow/model coverage is green
   in the recorded 20/20 Vitest run.
4. **FAIL.** `selectOperationalStatusTimestamp` matches only the state name; it does not apply
   `isExplicitAgentStatusFresh`. `selectLiveAgentStatusEntriesForWorktree` returns every entry in
   `agentStatusByPaneKey`, including stale ones, while the aggregate activity summary correctly
   ignores stale entries. Repro: leave a stale `blocked` entry in that map, then let watchdog
   `awaitingInputByPaneKey` or server `liveActivity: 'awaiting'` produce the winning `permission`
   status. The selector returns the stale entry's `stateStartedAt`, so the rich card renders a
   Needs input age belonging to a signal that did not win. The same mismatch can occur for a
   retained `done` winner plus a stale explicit done entry. The timestamp must be selected from
   the same freshness-qualified signal set as the aggregate winner, or omitted for fallback
   winners.
5. **PASS.** Settled now uses `compareSettledEntries`, which compares activity timestamp before a
   stable name tie-break and never checks `isPinned`. The regression fixture proves a newer
   unpinned workspace precedes an older pinned workspace; disclosure remains capped at three and
   preserves the full count.
6. **PASS by interaction-boundary inspection.** Both operational presentations still render
   through `WorktreeCard` and the existing virtual viewport callbacks, so no second activation,
   keyboard, context-menu, drag, selection, or reveal implementation was introduced. Real
   browser exercise remains part of the environment deferral below.
7. **PASS.** Absent/invalid persisted grouping normalizes to operational, supported explicit
   choices remain unchanged, legacy parent maps to host, and alternate grouping/options paths
   remain present.
8. **DEFERRED (environment).** Playwright MCP is absent, so the required 220 px / 500 px light
   and dark screenshots, keyboard traversal, visible-focus check, contrast audit, and runtime
   interaction exercise could not be captured. No screenshot or browser validation is claimed.
9. **PASS for the requested build/test commands.** The corrected recorded evidence is six
   focused Vitest files passing 20/20 and a production Vite build exiting 0 after 7,242 modules.
   The Tester independently reran the pure operational model with the available Bun runner and
   its eight tests passed. The long build was not rerun during this retest.

## Remaining required fix

Filter candidate explicit status entries with the same freshness rule and clock used by
`selectWorktreeAgentActivitySummary` before choosing a state timestamp, then add a regression
covering a fallback permission/done winner alongside a stale same-state pane entry. If the
winning source has no provably associated timestamp, omit the age.
