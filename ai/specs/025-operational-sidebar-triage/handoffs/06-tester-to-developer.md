# Tester → Developer Send-back (Iteration 2)

## 1. Summary

Settled ordering now passes. AC 4 still fails because status timestamp selection can use a
stale explicit entry even though the aggregate resolver correctly ignored that entry.

## 2. Completed Work

- Confirmed strict Settled recency and its regression.
- Re-ran the pure model suite: 8/8 passing.
- Audited explicit-status freshness against the authoritative activity summary.

## 3. Pending Work

- Apply `isExplicitAgentStatusFresh` with `AGENT_STATUS_STALE_AFTER_MS` and the same current
  clock before matching a pane timestamp.
- Add a regression for a fallback permission/done winner alongside a stale same-state entry.

## 4. Important Decisions

- A fallback winner has no pane timestamp; stale pane data must not be used as a substitute.

## 5. Risks

- This is the second send-back at the Tester gate. A third failure forces human review.

## 6. Questions

- None.

## 7. Recommended Next Step

Developer should make the minimal freshness fix and return it for a final Tester retest.

## Failed gate evidence

- **Gate item:** every acceptance criterion has a passing verdict. **Failed:** AC 4.
- **Evidence:** candidate entries are not freshness-filtered while the aggregate resolver is.
- **Fix direction:** use the authoritative freshness predicate/TTL and omit age for fallback-only
  winners.
