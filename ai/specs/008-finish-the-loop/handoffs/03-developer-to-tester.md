# Handoff 03 — Developer → Tester

- **Spec:** 008-finish-the-loop
- **Date:** 2026-07-03
- **From:** Developer (autonomous /sdd-loop, three developer iterations: F1 → F2 → F3)
- **To:** Tester
- **Artifacts:** `spec.md`, `architecture.md`, `tasks.md` (F1+F2+F3 sections), commits `51705bf2` (F1), `3b6dbd33` (F2), + F3 (this iteration, about to commit)

## Gate result

Developer gate: **PASS** for all three slices. The spec is **code-complete** —
F1 + F2 + F3 all landed, each built + tested green in its own iteration, F1/F2
suites confirmed still green after F2/F3.

## What to verify, per acceptance criterion

**F1 — the never-silent run path (`51705bf2`, AC 1–4).** The start-work wire
already shipped in spec 005; F1 closed four silences:
- **AC 1/2** #14a `await_repl_ready → bool` + #15 `wait_for_settle →
  SettleOutcome` (loud logs; the 1800 s hang is gone) + #2 the composer armed
  `!repoId` guard now toasts. AC 1's two-hop click path + the events bridge
  (`subscribeHarnessRunErrors`) surface a composer-started run's failure.
- **AC 3/4** live `status/*` flips + the new `status/blocked` label + comment
  on a red gate (`apply_blocked_transition`, GitHub-only, `Ok(Skipped)`-never-`Err`).
- **Verify:** `cargo test -p agentum-server -p agentum-executor --lib`
  (546/0/5 + 21/0 at F1; 552/0/5 after F2); the D6 argv tests + settle-outcome
  tests; UI vitest for the toasts.
- **⚠️ D5 human merge gate:** the sacred #14a change ships only with BOTH live
  tests green — `harness_live_agent.rs` **and** the new
  `harness_start_work_live{,_roles}.rs`. They spawn a **real claude agent** and
  CANNOT run in CI/autonomously → a **human pre-release step**, not the tester's
  autonomous gate. Confirm the binaries compile (`--no-run`).

**F2 — Fast / Complex chat intake (`3b6dbd33`, AC 5–8).**
- **AC 5** two composer buttons (Fast/Complex). **AC 6** Fast byte-identical
  (pinned by `build_intake_instructions_fast_equals_interviewer_verbatim`).
  **AC 7** staged five-pass Socratic (one pass/turn, never skips — pinned by the
  server per-stage test + the client `socratic-intake` reducer test). **AC 8**
  both converge on the unchanged `compose_issue_body`.
- **Verify:** `cargo test -p agentum-server --lib routes::chat::tests`;
  `npx vitest run src/lib/socratic-intake.test.ts`. No-creds surfaces by
  construction (`chat_auth_gate`, pinned hermetically — see F2 Deviation 1 for
  why it's the shared gate, not a live no-creds handler call).

**F3 — goal-first workspace (this iteration, AC 9–11).**
- **AC 9** goal step is the default first screen (`initialComposerPhase`), seeds
  name/prompt from the goal, "Skip to details" reveals today's composer (D3).
  **AC 10** goal + workdir required; worktree-creation / scaffold / tracker are
  three skippable steps (`OPTIONAL_WORKSPACE_STEPS`, D9). **AC 11** the wiring +
  seed/issue-draft logic reach `start_work`'s precondition set with no retyping.
- **Verify:** `npx vitest run src/lib/workspace-goal-step.test.ts` (15/0);
  `npm run build --prefix crates/agentum-desktop/ui` (vite + tsc).
- **AC 11 honesty:** wired + unit-tested; the full file-issue → scaffold →
  arm-gated-run → green demo is a **qa.sh / human browser check** (installed app
  + real repo).

## Known-clean caveats (do not misread as regressions)

- **Full `npx vitest run` shows 139 failed / 43 files.** This is a **proven
  pre-existing baseline** — identical count on `51705bf2` and `3b6dbd33` with
  zero spec-008 changes. Failures are in unrelated domains (sidebar color-class
  drift, git-status, settings, tab-bar, editor, unmocked Tauri `invoke`). Spec
  008 added **+34 passing, 0 new failures**. Only the four spec-008 test files
  import spec-008 modules; all pass. Verify by diffing the failing-set against
  the base commit if in doubt.
- **Desktop cargo crate not built** (needs sherpa dylibs in `target/release`);
  the gate is the server lib tests + vite build + vitest, all green.

## Sacred surfaces to spot-check clean

- F2/F3 did **not** touch F1's `drive.rs`/`harness.rs`/`task_sink.rs`/`git_fs.rs`
  or `useComposerState.ts` internals. F3 reused the composer via props only and
  kept F1's `initialStartGatedRunProp` spread. `interviewer_instructions` output
  is byte-identical; `compose_issue_body`/`spec_md_from_issue` untouched.

## Expected tester artifact

`ai/specs/008-finish-the-loop/verification.md` — pass/fail per AC 1–12 with
repro steps, the base-vs-head confirmation of the 139-failure baseline, and a
clear separation of what the autonomous gate proves vs what's deferred to the
qa.sh/human browser + the D5 live-test pre-release step.
