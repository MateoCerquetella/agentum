# Tester → Developer Send-back

## 1. Summary

The Tester gate failed on AC 4 and AC 5. The implementation is otherwise buildable and its
focused suites are green, but ordering and age truthfulness require shallow code fixes.

## 2. Completed Work

- Independently mapped all nine acceptance criteria to implementation and test evidence.
- Re-ran six focused suites: 17/17 passing.
- Confirmed the production build exits successfully.

## 3. Pending Work

- Fix Settled ordering to use recency before any pinned state.
- Couple displayed status age to the winning status signal, or omit the age if no truthful
  winning timestamp is available.
- Add/update focused regression tests for both cases.

## 4. Important Decisions

- These are Developer-level defects; no architecture or requirement change is needed.
- Browser-only responsive/theme evidence remains deferred because Playwright MCP is absent.

## 5. Risks

- Leaving the defects would make the queue ordering and status metadata contradict the spec.

## 6. Questions

- None.

## 7. Recommended Next Step

Developer should make the two minimal fixes, rerun focused tests and the production build,
then return the implementation to Tester.

## Failed gate evidence

- **Gate item:** every AC has a passing verdict. **Failed:** AC 4 and AC 5.
- **Evidence:** `compareOperationalEntries` applies pinned-first to Settled; `WorktreeList`
  chooses the newest agent timestamp independently from the aggregate winning status.
- **Fix direction:** specialize Settled comparison and make status/timestamp selection atomic.
