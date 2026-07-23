# Spec 027 — Implementation Tasks

- **Spec:** `027-remove-internal-workspace-board`
- **Architecture:** `architecture.md`
- **Status:** Reviewer fix complete — ready for reviewer retry

## F1 — Retire the internal-board desktop affordance

- [x] Remove the global Tasks sync callback, state, copy, and actions.
- [x] Delete the internal `/api/board/sync` client.
- [x] Preserve project-scoped GitHub/Linear views and clarify the configured-tracker empty state.
- [x] Extend the focused Tasks structural regression.
- **Acceptance criteria:** AC 1
- **Gate:** focused desktop tests and production UI build.
- **Gate evidence (2026-07-23):** corrected the stale deleted
  `SidebarHeader.test.tsx` path in `.agentum-harness/verify.sh`; exact F1 gate passed
  (`ProjectTasksPage.test.tsx`: 1 file, 4 tests), and the production UI build passed
  (7,253 modules transformed, 1m29s).

## F2 — Unregister and remove the internal-board server boundary

- [x] Move shared GitHub slug/error helpers out of deleted board modules.
- [x] Unregister and delete all internal `/api/board*` route families and board-only rules.
- [x] Delete retired integration suites that assert successful internal-board APIs.
- [x] Add a real-router 404 matrix covering family roots and representative nested paths.
- **Acceptance criteria:** AC 2, AC 4
- **Gate:** focused server route test and workspace compile/tests.
- **Gate evidence (2026-07-23):** exact F2 gate passed
  (`internal_board_route_families_are_unregistered`: 1 passed), and the workspace
  library suite passed.

## F3 — Make normal tracker creation and transitions external-only

- [x] Restrict creation sinks to GitHub and Linear.
- [x] Remove store access and the board arm from transition seams and all callers.
- [x] Preserve bounded best-effort handling for serialized legacy `provider: "board"` input.
- [x] Update MCP/harness comments and tests to advertise external providers only.
- **Acceptance criteria:** AC 3, AC 4
- **Gate:** focused task-sink/MCP/harness tests and workspace library suite.
- **Gate evidence (2026-07-23):** exact F3 gate passed (2 focused sink tests);
  `pinned_provider_dispatches_to_matching_tracker_arm` and
  `resolve_tracker_pin_maps_d4` each passed; the MCP `report_status` group passed
  7 tests; the workspace library suite passed.

## F4 — Remove runtime persistence and reconciliation while preserving old data

- [x] Remove board CRUD/binding APIs, core models, and board-only watchdog workers.
- [x] Leave historical migrations and compatibility session fields unchanged.
- [x] Prove legacy rows survive reopen and remain unchanged during normal store work.
- [x] Keep current API/data-model docs and embedded SDD playbooks external-tracker-only.
- **Acceptance criteria:** AC 3, AC 4
- **Gate:** focused store/watchdog/docs regressions and workspace library suite.
- **Gate evidence (2026-07-23):** exact F4 gate passed (legacy-row regression:
  1 passed; watchdog library: 11 passed); the docs/playbook compatibility regression
  passed; the workspace library suite passed.

## F5 — Run the green gate

- [x] Run all focused Rust and UI regressions.
- [x] Run `cargo test --workspace --lib`.
- [x] Run `npm run build --prefix crates/agentum-desktop/ui`.
- [x] Record any browser/runtime deferral explicitly; release promotion remains human-gated.
- **Acceptance criteria:** AC 1–5
- **Gate:** all required commands exit zero with no unresolved blocker.
- **Gate evidence (2026-07-23):** `cargo fmt --all -- --check` passed;
  `cargo test --workspace --lib` passed 901 tests with 6 expected ignores;
  the production UI build passed; focused results are recorded above. Browser/runtime
  QA of bound GitHub, bound Linear, and unbound Tasks states was not run in this
  developer phase and remains a Tester/release gate. The only observed warnings were
  the pre-existing server `forge_send` dead-code warning and Vite chunk/import advisories.

## Developer retry 1 — Make browser QA truthful

- [x] Replace the unconditional `qa.sh` pass with a named command seam covering
  GitHub-bound, Linear-bound, unbound, and unavailable Tasks scenarios plus the
  absence of internal-board cards and the `Sync to Board` action.
- [x] Exit 2 with explicit `PENDING/UNAVAILABLE` output when no real browser
  verifier is configured or the configured executable is unavailable.
- [x] Invoke the configured verifier with safely quoted feature-id and brief
  arguments, using `exec` so its pass/fail status propagates unchanged.
- [x] Update the stale `tracker_provider` comment to advertise GitHub/Linear while
  retaining `Option<String>` legacy-value deserialization.
- **Retry gate evidence (2026-07-23):** `bash -n` passed for `qa.sh` and
  `verify.sh`. The focused shell matrix passed: unset command → 2, unavailable
  command → 2, `/usr/bin/true` mock → 0, and `/usr/bin/false` mock → 1; required
  scenario and PENDING output assertions also passed. The focused Tasks UI suite
  passed 4/4, the legacy-provider Rust regression passed 1/1, `cargo fmt --all
  -- --check` passed, and `git diff --check` passed. Real browser execution
  remains explicitly PENDING/UNAVAILABLE because no verifier command is
  configured in this environment; it is no longer represented as a pass.

## Reviewer fix 1 — Make the truthful QA wrapper committable

- [x] Add the ordered `!qa.sh` exception to the spec-local
  `.agentum-harness/.gitignore` while retaining the leading `*` runtime-noise rule.
- [x] Confirm `git check-ignore .agentum-harness/qa.sh` no longer reports a match
  and `git status --short -- .agentum-harness/qa.sh` reports the wrapper as untracked.
- [x] Preserve the wrapper contract and rerun its unset, unavailable, pass, and
  fail status matrix plus required scenario/output assertions.
- [x] Rerun shell syntax and diff checks without changing product code.
- **Reviewer-fix evidence (2026-07-23):** the visibility checks passed: `git
  check-ignore` exited 1 with no output, and targeted status reported `??
  .agentum-harness/qa.sh`. The wrapper matrix remained 2/2/0/1; assertions passed
  for every named scenario, the retired-affordance requirement, both unavailable
  messages, and the exact two-argument verifier contract. `bash -n` passed for
  `qa.sh` and `verify.sh`; `git diff --check` passed. The leading `*` rule still
  ignores other untracked harness runtime files. After `qa.sh` is added to version
  control, that ignore rule cannot untrack it in a fresh checkout.
