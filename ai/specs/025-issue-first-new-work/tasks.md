# Tasks — Spec 025: Issue-first New Work

## F1 — Deferred new-issue intent

**Implemented.** The wizard stages New/Existing intent, files New only from the
final action, checkpoints the confirmed issue, and uses contextual launch copy.

- Add the React-free New Work source/execution/stage model and focused tests.
- Render New issue / Existing issue as mutually exclusive wizard choices.
- Keep title, body, labels, and AI draft editable; remove early file button and
  Enter-to-file from the wizard variant.
- Return the confirmed `LinkedWorkItemSummary` from issue creation and pass it as
  an explicit `submitQuick` override.
- Render contextual final CTA and derive the worktree name from the confirmed
  issue title.
- **Covers:** AC 1–2 and the source half of AC 8.

## F2 — Issue-backed spec invariant

**Implemented.** The route/client expose opt-in converge, and explicit manual
and Autopilot quick-submit branches both prepare the issue-derived spec while
leaving legacy unoptioned composer behavior intact.

- Add optional `converge` request / `specExisted` response semantics to
  `POST /api/harness/spec-from-issue`, keeping the default 400 contract.
- Widen the desktop client with `plan:false, converge:true` support.
- Replace the wizard's gated checkbox with SDD Autopilot / Open manually.
- Autopilot calls existing `start-work`; manual local-GitHub mode prepares the
  spec before opening the unchanged plain-agent path.
- Keep unoptioned composer/Tasks behavior backward-compatible.
- **Covers:** AC 3–6.

## F3 — Explicit execution and recovery

**Implemented.** Modal-lifetime issue/worktree checkpoints drive retry reuse,
ordered progress, field locking after durability, and strict Autopilot
ownership (no plain-agent fallback).

- Checkpoint confirmed issue and full worktree result immediately after each
  irreversible success.
- Reuse checkpoints on Retry; lock fields after their durability boundary.
- Render Issue → Worktree → Spec → Run progress and precise stage errors.
- Make Autopilot strict: ownership or visible failure, never plain fallback.
- Surface surviving worktree actions after post-create failure.
- Add call-order/count tests and installed-app forced-failure QA coverage.
- **Covers:** AC 7–8 and reinforces AC 5's one-driver invariant.

## Gate commands

- Focused `bunx vitest run` set named in `architecture.md`.
- `npm run build --prefix crates/agentum-desktop/ui`.
- `cargo test -p agentum-server --lib routes::harness::tests`.
- `git diff --check`.
- Installed desktop `qa.sh` scenarios from the spec/architecture.

## Developer gate results

- Focused Vitest: **PASS**, 6 files / 106 tests.
- `git diff --check`: **PASS**.
- Vite production build: **PASS** — 7,239 modules transformed in 2m41s.
- Rust focused converge test: **PASS** — 1 passed, 0 failed, 787 filtered.
- Installed-app `qa.sh`: **DEFERRED** to Tester/staging as specified.
