# Handoff — Developer to Reviewer

- **Spec:** 027-remove-internal-workspace-board
- **From:** Developer
- **To:** Reviewer
- **Date:** 2026-07-23
- **Gate:** **PASS (Reviewer fix iteration 1)**

## Delivered

- Added only the ordered `!qa.sh` exception beneath the spec-local harness `*`
  rule. The truth-preserving wrapper is now visible to normal version-control
  collection, while other untracked harness runtime noise remains ignored.
- Recorded the bounded Reviewer-fix evidence in `tasks.md`.
- Preserved product code, the global scaffold policy, `ai/STATE.md`, unrelated
  worktree changes, and the existing Vitest cache content.

## Acceptance-criteria evidence

- **AC 1:** the repaired QA wrapper is now an untracked, committable artifact;
  its required scenario assertions and 2/2/0/1 propagation matrix still pass.
- **AC 2–5:** unchanged by this metadata-only fix; the Reviewer already found
  their product behavior and evidence green.

## Verification

- `git check-ignore .agentum-harness/qa.sh` — **PASS** for visibility (exit 1,
  no matching ignore rule).
- `git status --short -- .agentum-harness/qa.sh` — **PASS** (`??
  .agentum-harness/qa.sh`).
- QA unset/unavailable/true/false matrix — **PASS** (2/2/0/1).
- QA named-scenario, retired-affordance, unavailable-output, and exact argv
  assertions — **PASS**.
- `bash -n .agentum-harness/qa.sh .agentum-harness/verify.sh` — **PASS**.
- `git diff --check` — **PASS**.

## Decisions and invariants

- This is a spec-local collection exception, not a change to the global harness
  scaffold template.
- The leading `*` continues to suppress untracked runtime noise. Once `qa.sh` is
  added to version control, that surrounding ignore rule does not untrack it in
  fresh checkouts.
- No browser result was manufactured: real-browser QA remains the explicit
  environment/release gate described by the existing wrapper and evidence.

## Remaining risks / next action

- Reviewer should confirm the QA wrapper is included when the change set is
  staged/committed, then retry sign-off. Release and live external mutations
  remain human-gated.
