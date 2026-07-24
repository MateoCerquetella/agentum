# Spec 027 — Review

- **Date:** 2026-07-23
- **Role:** Reviewer
- **Iteration:** 2
- **Verdict:** SIGN-OFF — ship-ready; release remains human-gated

## Blockers

None. Reviewer iteration 1's portability blocker is closed: the spec-specific truthful
`.agentum-harness/qa.sh` is visible to normal version-control collection while other harness
runtime noise remains ignored.

## Acceptance-criteria disposition

- **AC 1 — PASS:** `ProjectTasksPage.test.tsx` passes 4/4, source dispatches only configured
  GitHub/Linear views or explicit unbound/unavailable states, and the desktop build is green. The
  QA wrapper names all four browser scenarios and refuses to pass without evidence. Real browser
  execution is explicitly PENDING for the human release gate, not reported green.
- **AC 2 — PASS:** the real router's focused test proves 404 for every retired family root and
  representative nested paths.
- **AC 3 — PASS:** creation sinks are GitHub/Linear-only, transition signatures are store-free,
  provider dispatch/MCP regressions pass, and legacy `board` input is a bounded non-writing skip.
- **AC 4 — PASS:** the reopen regression proves a seeded legacy row remains byte-identical around
  normal store work; board CRUD, binding, reconcilers, and runtime routes remain absent while
  historical migrations stay unchanged.
- **AC 5 — PASS:** Developer and independent Tester runs each report 901 passed, 6 ignored, 0
  failed for `cargo test --workspace --lib`; both desktop builds transformed 7,253 modules and
  exited zero. Focused F1–F4, formatting, syntax, wiring, and diff checks are green.

## Reviewer-fix verification

- Plain `git check-ignore .agentum-harness/qa.sh` — PASS for visibility (exit 1, no output).
- `git status --short -- .agentum-harness/qa.sh` — PASS (`?? .agentum-harness/qa.sh`).
- QA unset/missing/pass/fail propagation — PASS (`2/2/0/1`).
- Named-scenario, exact quoted argv/brief, unavailable-output, and shell-safety assertions — PASS.
- `bash -n .agentum-harness/qa.sh .agentum-harness/verify.sh` — PASS.
- `git diff --check` and retired-runtime-seam search — PASS.

## Invariant and security review

- No migration rewrite, durable-row deletion, or live tracker mutation occurred.
- GitHub Projects, Linear kanban, harness-board terminology, one launch path, push streaming, and
  tracker best-effort behavior remain intact.
- The verifier is an executable path/name passed two quoted arguments; no shell source is evaluated,
  and its exact nonzero status propagates.
- Legacy tracker strings remain deserializable while current comments advertise external providers
  only.

## Should-fixes and release gate

- Before release, execute GitHub-bound, Linear-bound, unbound, and unavailable Tasks scenarios with
  a real browser verifier and safe fixtures. Keep any inconclusive result nonzero.
- Include the now-visible `.agentum-harness/qa.sh` with the change set.
- The pre-existing `forge_send` dead-code and Vite chunk/import warnings are non-blocking.
- Merge, staging promotion, live tracker writes, and release are not authorized by this sign-off.
