# Handoff — Tester to Reviewer

- **Spec:** 025-issue-first-new-work
- **From:** Tester
- **To:** Reviewer
- **Date:** 2026-07-22
- **Gate:** PASS WITH DECLARED INSTALLED-APP QA DEFERRALS

## Independent result

All eight acceptance criteria have implementation and executable evidence. No
blocker or should-fix implementation defect was found.

- Focused UI: **106/106 PASS** across 6 files, including the B1 keyboard-gate
  regression.
- Vite production build: **PASS**, 7,239 modules transformed.
- Focused Harness server tests: **10/10 PASS**.
- Rust formatting and diff hygiene: **PASS**.

## Reviewer focus

1. Confirm the one-owner invariant: explicit Autopilot cannot reach the plain
   agent fallback, while Manual prepares the spec and opens exactly one plain
   agent.
2. Confirm checkpoint durability ordering around the confirmed GitHub issue and
   full worktree result, including the same-frame launch guard.
3. Confirm Reviewer B1 stays closed: `canLaunchNewWork` is shared by the button
   disabled state and `handlePrimary`, blocking Enter as well as click when the
   selected agent is unavailable or setup is unresolved.
4. Review the compatibility boundary: unoptioned composer calls retain the old
   `scaffoldSpec` / `startGatedRun` behavior, and `spec-from-issue` converge is
   opt-in rather than a global contract change.
5. Note the verification-coverage limitation: full coordinator call-order and
   worktree retry reuse are proven by boundary inspection, not a dedicated
   hook-level fake-dependency test.

## Release/staging gate still required

Installed-app `qa.sh` was **not run** because this worktree has no available
scratch GitHub/app/fault-injection environment. Before release, exercise both
execution modes and forced failures to confirm exactly one issue, one worktree,
one generated spec, the correct single owner/session, ordered progress, no
duplicate issue/worktree on Retry, honest ineligible copy, and usable layout at
the supported minimum height.
