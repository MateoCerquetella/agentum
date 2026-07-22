# Verification — Spec 025: Issue-first New Work

- **Role:** Tester
- **Date:** 2026-07-22
- **Verdict:** PASS WITH DECLARED INSTALLED-APP QA DEFERRALS
- **Implementation defects:** 0 blocker, 0 should-fix

## Executable gates

| Gate | Result | Evidence |
|---|---|---|
| Focused desktop UI tests | PASS | `bunx vitest run --config vite.config.ts ...` from `crates/agentum-desktop/ui`: 6 files, 106 tests passed. The set covers the new launch model plus wizard, create-intent, issue-side-effect, ownership, and workspace-opening contracts, including the Reviewer B1 keyboard-gate regression. |
| Desktop production build | PASS | `npm run build` from `crates/agentum-desktop/ui`: Vite transformed 7,239 modules and completed successfully. Existing chunk-size/dynamic-import warnings were non-fatal and unrelated to this slice. |
| Focused Harness server tests | PASS | `cargo test -p agentum-server --lib routes::harness::tests`: 10 passed, 0 failed, 778 filtered. This includes default/opt-in converge, fresh/existing-spec behavior, planning, tracker status, settings wire, and start-work knob tests. |
| Rust formatting | PASS | `cargo fmt --all -- --check`. |
| Diff hygiene | PASS | `git diff --check`. |
| Installed desktop `qa.sh` | UNRUN | No spec-local `qa.sh`/installed-app scratch GitHub environment or deterministic post-issue/post-worktree fault injector is available in this worktree. The live issue/worktree/session assertions below remain an explicit staging/human gate. They are not labeled passing. |

## Acceptance-criterion evidence

### AC 1 — mutually exclusive deferred New/Existing source

**PASS, with rendered interaction deferred.** `CreateWorkspaceWizard` owns a
`WorkSource` union and renders mutually exclusive New issue / Existing issue
buttons. The New panel retains title, description, AI draft, and GitHub label
controls. Its file button is absent in deferred mode and Enter only stops
propagation, so neither path files before the final primary action. Existing
continues to render the project-scoped picker. The pure suite covers the source
branches; installed-app confirmation of the rendered controls is unrun.

### AC 2 — contextual final action and exactly one issue

**PASS, with live GitHub count deferred.** `newWorkPrimaryLabel` produces
`Create issue & start work` versus `Create worktree & start work` and is covered
by the focused suite. `resolveLaunchIssue` calls creation only for New, stores the
confirmed summary before continuing, reuses a checkpoint on Retry, and has a
test proving one create call across retry. Existing issue has a test proving
zero create calls. The explicit summary override supplies issue number/title to
`submitQuick`; the existing editable name remains authoritative when supplied,
otherwise the confirmed title seeds the worktree name. Live GitHub/worktree
cardinality remains in installed-app QA.

### AC 3 — explicit execution outcome and product copy

**PASS, with rendered layout deferred.** The wizard renders mutually exclusive
SDD Autopilot / Open manually controls. Eligible work initializes to Autopilot,
and the visible Autopilot copy names `PM → Architect → Build → Verify → Review`.
The replaced primary controls expose none of `Harness`, `scaffold`, or `gated
run`. Eligibility/default/label derivations pass focused tests. Minimum-height
layout and visual usability remain installed-app QA.

### AC 4 — issue-backed spec invariant

**PASS for the local GitHub boundary; filesystem observation deferred.** Both
explicit execution branches run after the linked worktree exists. Autopilot
calls `startGatedWork`, whose tested server path converges the issue spec before
planning/driving. Manual derives the same local-GitHub gate and calls
`scaffoldSpecFromIssue({ plan:false, converge:true })` before opening the agent.
The New Work surface no longer presents the standalone scaffold choice. The
production build validates the integrated client types; actual filesystem
presence in an installed worktree remains unrun.

### AC 5 — strict Autopilot ownership

**PASS, with live single-agent count deferred.** The explicit Autopilot branch
calls the existing `startGatedWork`, passes selected agent/model and tracker
coordinates, and requires `gatedRunResultOwnsWorktree`. A zero-owner response
throws. Only confirmed ownership reaches `openCreatedWorkspace` with
`gatedRun:true`, whose focused ownership/opening tests prove the plain agent path
is suppressed. Errors stay in the catch/progress path; there is no explicit-mode
fallback through `maybeStartGatedRun`. Live process/session counting is unrun.

### AC 6 — manual converge then one plain agent

**PASS, with live session count deferred.** Manual local-GitHub work calls
`spec-from-issue` with `plan:false, converge:true`, then uses the established
`openCreatedWorkspace({ gatedRun:false })` activation. The server's 10-test
Harness set proves converge keeps an existing spec and legacy absent/false
semantics remain opt-in. The implementation does not start the Harness driver in
this branch. Human-edited-byte preservation is covered by the existing-spec
server test; live one-agent/no-driver observation is unrun.

### AC 7 — ordered progress and resumable irreversible steps

**PASS at boundary/model level; forced-failure UI QA deferred.** Progress is a
typed Issue → Worktree → Spec → Run record rendered in that order. The confirmed
issue is checkpointed before `submitQuick`; the full `CreateWorktreeResult` is
published immediately after creation and reused on Retry. The new same-frame
`launchInFlightRef` guard plus composer busy state closes the double-click/Enter
race. The source/issue inputs lock after issue durability and name/agent/mode
lock after worktree durability. A retry model test proves the issue is not filed
twice; static inspection proves a checkpointed worktree bypasses
`createWorktree`. The modal displays completed Worktree progress and keeps Retry
available instead of rolling anything back. Deterministic post-issue and
post-worktree failure exercises remain installed-app QA.

### AC 8 — honest eligibility and no silent degradation

**PASS, with rendered matrix deferred.** The pure eligibility model returns
specific remote, non-git, non-GitHub, unavailable-agent, and setup-blocked
outcomes. The focused matrix passes. The wizard renders the reason before
submission, disables Autopilot, and requires the operator to choose Manual.
Reviewer B1 is closed: `canLaunchNewWork` is now the single predicate consumed
by both the disabled primary button and `handlePrimary`, so keyboard submission
cannot bypass unavailable-agent or setup blockers. A regression test proves
both blockers fail closed while an explicitly selected compatible remote Manual
path remains allowed. The empty-agent copy now says an agent must be installed
or detected before starting. Manual incompatible paths explicitly say `no
generated SDD spec`. The explicit Autopilot branch throws rather than falling
through to a plain session. Installed-app rendering for every matrix row is
unrun.

## Negative, race, and error review

- New-issue failure leaves the draft intact and marks Issue error.
- Mouse and keyboard launch paths share `canLaunchNewWork`; Manual mode cannot
  file an issue while agent availability or setup is blocked.
- The same synchronous launch cannot pass `launchInFlightRef` twice; the normal
  asynchronous composer busy state also disables the primary button.
- A confirmed issue is captured before the worktree call, eliminating stale
  React closure dependence and duplicate filing on later retry.
- A created worktree is checkpointed before metadata/spec/run calls; retry
  reuses its setup/default-tab result rather than creating another worktree.
- Autopilot ineligibility and no-owner results are fail-closed.
- Manual converge is retry-safe and retains existing spec contents.
- Inputs crossing an irreversible boundary are disabled for the remainder of
  the modal lifetime.
- Durable recovery after modal/app close is intentionally outside this spec.

## Verification limitation for Reviewer

The architecture requested fake-dependency coordinator tests for full
create-worktree/spec/start call ordering. The submitted focused suite directly
tests issue checkpoint call counts and the constituent ownership/opening gates,
while the full `submitQuick` ordering/reuse proof is boundary inspection plus a
green production build rather than a dedicated hook-level fake test. This is a
non-blocking coverage limitation, but Reviewer should treat the declared
installed-app forced-failure scenarios as mandatory before release.
