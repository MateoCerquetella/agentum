# Handoff — Tester to Developer

- **Spec:** 028-bound-transcript-observers
- **From:** Tester
- **To:** Developer
- **Date:** 2026-07-23
- **Gate:** SEND-BACK (Tester iteration 1 of 2)

## Delivered

- Independent AC 1–8 trace and executable gate run in `verification.md`.
- Green focused, isolated-QA, formatting, diff, and non-desktop workspace evidence.
- Three bounded verification defects for AC 6, AC 8, and the stated QA contract.

## Acceptance-criteria evidence

- **AC 1–5, 7:** PASS with focused and workspace evidence.
- **AC 6:** BLOCKED. Store lifecycle methods are tested directly, but stop/kill/delete production
  routes and the server-wired watchdog retirement callback lack deterministic executable tests.
- **AC 8:** BLOCKED. Source uses capacity-one `try_send` and abort-on-drop, but the fake factory
  cannot trigger callbacks or observe consumer termination, so coalescing, queued-wake silence,
  and prompt shutdown remain unproved.

## Verification

- Focused Spec 028 suites — PASS (7 store + 2 agent-task + 2 session + 1 watchdog).
- Isolated `.harness/qa.sh` route — PASS as implemented, but its output overstates runtime scope.
- `cargo test --workspace --lib --exclude agentum-desktop` — PASS (829 passed, 2 ignored).
- `cargo fmt --all -- --check` and `git diff --check` — PASS.
- Full workspace and UI build — unchanged environment blockers (Sherpa dylib; Vite absent).

## Decisions and invariants

- Do not weaken AC 6/8 or relabel source inspection as executable proof.
- Tests must exercise production route/wiring seams and a controllable observer callback/consumer,
  while preserving the capacity-one channel, atomic store decision, and no-stale-event invariant.
- QA output must state only what the isolated gate actually measures; implement the promised
  runtime leg or narrow its claim without pretending unit fixtures count OS threads/observers.

## Remaining risks / next action

- Add deterministic stop/kill/delete and server-hook retirement regressions with create/drop/cache
  counts and no-start assertions.
- Extend the injected observer test seam to trigger notify bursts and observe consumer completion;
  prove coalescing and no post-retirement update.
- Make the Spec 028 QA branch truthfully execute/report its coverage, then rerun all Tester gates.
