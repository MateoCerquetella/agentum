# Durable report export

## Goal

Users can export a stable report without losing the current workspace.

## Acceptance Criteria

- [ ] [AC-1] A user can export a durable report.
- [ ] [AC-2] An export failure leaves the existing report unchanged.

## Scope

Add one deterministic local export path.

## Non-goals

Cloud publication and scheduled exports are out of scope.

## Verification

Run the focused export contract tests.
