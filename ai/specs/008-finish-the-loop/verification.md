# Spec 008 — Finish the Loop — Verification Report

- **Spec:** `ai/specs/008-finish-the-loop/spec.md` (AC 1–12, D1–D9)
- **Tester:** sdd-tester (autonomous re-run of every gate; independent diff audit)
- **Date:** 2026-07-03
- **HEAD commit:** `9423b86f` (worktree `finish-the-loop`, clean tree)
- **Under test:** `51705bf2` (F1 never-silent run path) · `3b6dbd33` (F2 Fast/Complex intake) · `9423b86f` (F3 goal-first workspace)
- **Toolchain:** cargo 1.94.1 (`~/.cargo/bin`), node v22.14.0, vitest v4.1.8

## Verdict summary

**PASS-WITH-DEFERRALS.** Every autonomously-verifiable gate is green with the developer's claimed numbers reproduced exactly. Every deviation (F1×4, F2×4, F3×3) is accurate against the code. Every sacred surface is clean. The 139-failure vitest baseline is corroborated as pre-existing via four independent methods. No defect found: no AC's code contradicts its claim, no claimed test is hollow or missing, no gate is red, no invariant is broken. The only remaining gaps are the by-design deferrals the spec itself names — the `qa.sh` browser gate, the D5 real-claude live tests (human pre-release), and the AC-12 installed-app demo (Mateo). Hand off to the **reviewer**.

---

## 1. Re-run gate results (real observed numbers)

| Gate | Command (PATH=`~/.cargo/bin:$PATH`) | Result | Dev claim | Match |
|---|---|---|---|---|
| Backend lib | `cargo test -p agentum-server -p agentum-executor --lib` | **agentum-server 552 passed / 0 failed / 5 ignored** (72.8s); **agentum-executor 21 / 0 / 0** | 552/0/5 + 21/0 | ✅ |
| Spot: F2 chat | `cargo test -p agentum-server --lib routes::chat::tests` | **47 passed / 0 failed** | — | ✅ runs |
| Spot: F1 D6 | `cargo test -p agentum-server --lib task_sink::tests` | **32 passed / 0 failed / 1 ignored** | — | ✅ runs |
| Spot: settle | `cargo test -p agentum-server --lib -- settle_returns` | **2 passed / 0 failed** | — | ✅ runs |
| Spot: 5 named F2/D6 pins | (by name) | **5 passed / 0 failed** — `build_intake_instructions_fast_equals_interviewer_verbatim`, `socratic_stage_prompts_cover_one_pass_each_and_converge_at_five`, `chat_auth_gate_surfaces_no_creds_when_unauthed`, `apply_blocked_transition_board_and_linear_are_skipped`, `github_mark_blocked_with_fake_gh` | — | ✅ |
| Format | `cargo fmt --all --check` | **exit 0 (clean)** | clean | ✅ |
| Lints | `cargo clippy -p agentum-server --tests` | **exit 0, 0 warnings/0 errors** | 0 warnings | ✅ |
| Live-test compile | `cargo test ... --test harness_start_work_live --test harness_start_work_live_roles --test harness_live_agent --no-run` | **all 3 binaries compiled**; all carry `#[ignore]` → **not run in the gate** | compiles | ✅ |
| UI build | `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix crates/agentum-desktop/ui` | **✓ built in 1m20s (vite), exit 0, 0 `error TS` in log** (see Info #1 re: tsc) | vite+tsc green | ✅ (nuance) |
| Spec-008 vitest (5 files) | `npx vitest run src/lib/{workspace-goal-step,socratic-intake,start-gated-run-precondition,composer-modal-props,issue-side-effect-gate}.test.ts` | **34 passed / 0 failed (5 files)** | 34/0 | ✅ |
| Full vitest (diligence) | `npx vitest run` | **139 failed / 5761 passed (5900 tests); 43 failed / 700 passed (743 files)** | 139 fail / 5761 pass | ✅ |

All 10 claimed new/renamed D6 + settle test functions exist in source (`grep` count = 1 each). All 3 live tests carry `#[ignore = "spawns a real claude agent…"]`.

---

## 2. Per-AC verdict (AC 1–12)

Legend: **PASS** = verified now · **PASS (deferred)** = impl + unit/pin green, full runtime behavior owned by a named human/browser gate · **FAIL** = defect.

| AC | Criterion (short) | Verdict | Evidence / repro |
|---|---|---|---|
| **1** | Start gated run → visible ack ≤2s + session-or-actionable-error ≤15s; never silent (incl. #226 `repoId:''`) | **PASS (deferred)** | Unit/pin green: armed `!repoId` guard now `toast.error(firstStartGatedRunBlocker(...))` (`useComposerState.ts:2353`), server `ApiError` detail appended to the failure toast, `subscribeHarnessRunErrors` bridge (`harness-client.ts:378`, fires once/self-closes). Pins: `start-gated-run-precondition.test.ts` (4, incl. `''` edge), `composer-modal-props.test.ts` (2), `issue-side-effect-gate.test.ts` "distinct non-empty message for **every** skip reason". Live click→visible-session behavior owned by **qa.sh** + `harness_start_work_live.rs` (asserts `runStarted`+`AgentSpawned`). |
| **2** | Spec-grounded prompt visible in pane; non-empty output ≤60s (settle window) | **PASS (deferred)** | Unit green: `await_repl_ready → bool` + `wait_for_settle → SettleOutcome{Settled,Crashed,TimedOut}` + loud `repl_not_ready_message()`/`settle_timeout_message()` at 4 drive sites. Tests: `settle_returns_after_grace_on_early_idle`==Settled, `settle_ignores_events_for_other_sessions`==TimedOut, `settle_returns_crashed_on_session_stop`==Crashed. Live prompt-lands-in-pane owned by `harness_start_work_live.rs` (`support_start_work/mod.rs:234` asserts the `MARKER` reaches the pane via `capture-pane -p -S -`) — **D5 human pre-release**. |
| **3** | Loop drives; `status/*` flips live (todo→in-progress→ready-to-test→done) | **PASS (deferred)** | `apply_tracker_transition` seam intact; the live test asserts `--add-label status/todo` (plan) and `--add-label status/in-progress` (spawn) against a fake `gh` (`support_start_work/mod.rs:240,246`). Full four-stage flip owned by qa.sh + D5. |
| **4** | Red/blocked loudly surfaced in-app **and on the issue** (comment w/ retry+gate-tail + `status/blocked`) | **PASS (deferred)** | `apply_blocked_transition` wired in `handle_gate_failure` blocked branch (`drive.rs:312`); D6 argv + comment builders unit-pinned: `gh_set_blocked_label_argv_adds_blocked_removes_four_pipeline`, `blocked_comment_body_carries_attempts_and_gate_tail`, `github_mark_blocked_with_fake_gh` (asserts ensure+edit+comment), `apply_blocked_transition_board_and_linear_are_skipped`, one-of-five remove-set. Visual issue-comment owned by qa.sh. |
| **5** | Composer renders **Fast feature** + **Complex feature** buttons | **PASS** | `ChatPage.tsx`: `onClick={() => submitWith('fast')}` `<Zap/> Fast feature`; `onClick={() => submitWith('socratic')}` `<Brain/> Complex feature`; continuing-socratic "pass N of 5" indicator. Vite build green. Pixel placement is qa.sh but the wiring+labels are confirmed. |
| **6** | Fast byte-identical to today's prompt | **PASS** | `build_intake_instructions_fast_equals_interviewer_verbatim` asserts `assert_eq!` across stages [0,1,3,5,9,250] + no-grounding. Refactor is assembly-only: `intake_grounding_blocks` returns the tuple (construction unchanged), `interviewer_instructions` unpacks + runs the identical `format!` (unchanged context). Guarded also by `interviewer_grounds_when_context_present` + `interviewer_is_honest_blind_when_no_context`. |
| **7** | 5-pass Socratic; exactly one pass/turn, never skips; per-stage prompts; converge at 5 | **PASS** | Server pin `socratic_stage_prompts_cover_one_pass_each_and_converge_at_five` asserts each stage's single topic (WHO/WHAT/WHY/DONE-CRITERIA/RISKS), reflect-back on 2–5, only stage 5 has "Preview issues"+"STOP asking questions". Client reducer `advanceIntake` (Fast never advances; Socratic `clampStage(stage+1)`, cap 5) + `socratic-intake.test.ts` `[1,2,3,4,5,5,5]`. Reflect-back *rendering* is QA-observed per spec. |
| **8** | Both modes end at SDD-shaped issue bodies (`compose_issue_body` round-trip) | **PASS** | `compose_issue_body`/`spec_md_from_issue` **untouched** by F2 (diff grep empty); convergence by construction (stage 5 → existing Preview-issues path). Their round-trip tests unchanged-green. |
| **9** | Goal input is the first step; no repo/branch required before goal | **PASS** | `initialComposerPhase({})==='goal'`; modal `phase` lazily init from it; `onOpenAutoFocus` focuses `#workspace-goal`. Seed mapping (`deriveWorkspaceGoalSeed`, `slugifyGoalName`) + reveal pinned in `workspace-goal-step.test.ts` (15). |
| **10** | worktree/scaffold/tracker optional & skippable; goal+workdir the only required | **PASS** | `isGoalStepReady({goal,repoId})` requires both; `firstGoalStepBlocker` names the unmet input (never silent); `OPTIONAL_WORKSPACE_STEPS` = exactly 3 skippable, each naming its reused primitive (`createWorktree`/`maybeScaffoldSpecFromIssue`/`createGithubIssue`). "Skip to details" reveals composer with `seed=null`. |
| **11** | All-accepted workspace can run criteria 1–8 with zero further setup | **PASS (deferred)** | Wiring + pure logic delivered: goal seeds name+workdir+create-issue form (seed-gated one-shot `useEffect` via public `onCreateIssueTitleChange`/`onCreateIssueBodyChange`), reaching `start_work`'s precondition set. Full file-issue→scaffold→arm→green-gate run is **qa.sh / human** (installed app + real repo). |
| **12** | One unbroken demo in the **installed release app** | **PASS (deferred → Mateo)** | Release-gate demo, human-gated by standing convention. Evidence artifact = the issue's label-flip + harness-comment trail + an `ai/STATE.md` decision-log line. Not autonomously exercisable. |

**No AC scored FAIL.** AC 1–4's live runtime is deferred to qa.sh + the D5 live tests; AC 5–10 are verified now; AC 11 wiring is verified, its end-to-end run deferred to qa.sh; AC 12 is the human release demo.

---

## 3. Baseline verification (the 139-failure claim)

**Method (I cannot `git checkout` the base, so I corroborate via four independent prongs):**

1. **Disjoint set.** The 5 spec-008 test files all PASS (34/0) and **none** appears in the 43 failing files (extracted from the full log). ✅
2. **No reference.** Scripted grep of all 43 failing files for any of the 13 spec-008-touched source-module names → **zero hits**. The failing tests don't even mention a spec-008 module. ✅
3. **Diff scope.** The complete touched-source list (25 files) contains **no** file in any failing domain — spec-008 changed only composer/workspace-creation, chat, and harness/task_sink files; the failures are in sidebar/git-status/settings/tab-bar/editor/terminal-pane/onboarding/pane-manager/store-slices/web-preload/routing. ✅
4. **Failure kind.** Sampled failure reasons are all environmental/infrastructure, none spec-008: `Failed to resolve entry for "@xterm/addon-ligatures"`, `Cannot find package "@xterm/headless"`, `Cannot find module ".../main/ipc/worktree-logic"`, `ENOENT .../renderer/src/App.tsx`, `TypeError: api.ui.onRequestSplitRatio is not a function` (unmocked Tauri surface). These match the pre-existing categories documented in project memory. ✅

**Result:** The 139-failure baseline is **corroborated pre-existing**. Spec 008 added **+34 passing tests, 0 new failures**. The observed 139/43 matches the developer's F3-iteration claim exactly.

---

## 4. Deviation audit (accurate / inaccurate)

**F1 (4):**
1. *Loud settle log at 4 sites, not 3* — **ACCURATE.** Diff shows the `settle_timeout_message` log at `drive_inner`, `handle_gate_failure` retry, `run_qa_agent_gate`, and `run_role_gate` (the 4th uses `None` feature id). Every `wait_for_settle` caller must consume the new return type; strictly more correct.
2. *`gh_in_dir` honors `AGENTUM_GH_BIN`* — **ACCURATE.** `HostKind::Local` arm only; `std::env::var("AGENTUM_GH_BIN").unwrap_or_else(|_| "gh".into())` → production byte-identical.
3. *Minimal `initialStartGatedRunProp` extraction in `NewWorkspaceComposerModal.tsx`* — **ACCURATE.** Pure one-liner `modalData?.startGatedRun ? { initialStartGatedRun: true } : {}`; behavior-identical to the old inline spread; kept intact by F3 (line 195).
4. *`record_feature_failure` return-widen to `(bool, u32)`* — **ACCURATE.** Signature `Result<(bool,u32)>`; two result-binding tests updated to assert attempts (1, then 2 at cap); statement-form callers discard the tuple.

**F2 (4):**
1. *No-creds pin tests the shared `chat_auth_gate`, not a live handler* — **ACCURATE + well-justified.** macOS Keychain fallback makes a live no-creds test non-hermetic; `chat_auth_gate_surfaces_no_creds_when_unauthed` asserts `None ⇒ BadRequest(NO_CREDS_MSG)`, `Some ⇒ passthrough`. Guards the real invariant (Complex has no bypassing endpoint).
2. *`chat`'s inline no-creds literal unified onto `NO_CREDS_MSG`* — **ACCURATE.** Behavior-preserving dedup (same bytes).
3. *Grounding extracted into `intake_grounding_blocks` (assembly-only)* — **ACCURATE.** Verified byte-identical via the diff (format body unchanged context) + the Fast pin + the two content tests.
4. *`mode`/`stage` also added to `sendChat`, not only `streamChat`* — **ACCURATE.** Both serde-default server-side; Fast/old-client wire unchanged; build green.

**F3 (3):**
1. *Opinionated-open gate (`shouldStartAtGoalStep`/`initialComposerPhase`)* — **ACCURATE.** A pure, unit-tested predicate; an opinionated open (F1 Tasks hop, linked item, prefilled name, pinned branch) skips to `details`, keeping F1's hop byte-identical.
2. *"Pre-offer" = visible list + seeded create-issue form, not auto-run* — **ACCURATE.** Seed-gated one-shot `useEffect` calls the composer's **public** `onCreateIssueTitleChange`/`onCreateIssueBodyChange`; form stays closed/skippable; "Skip to details" (seed=null) is byte-identical.
3. *Goal-step workdir reuses `RepoCombobox`, not full host-scoping* — **ACCURATE.** Reuses the exact component with the same `Boolean(repo.path)` filter; never instantiates `useComposerState` twice; host scoping stays in the revealed composer.

---

## 5. Sacred-surface check (all clean)

- **F1 YOLO marker push + `spawn_agent_into_pane`** — untouched (grep of the F1 drive.rs diff for `YOLO_MARKER|spawn_agent_into_pane|translate_yolo` = empty). ✅
- **`apply_blocked_transition` is `Ok(Skipped)`-never-`Err`** — no `?`, `bail!`, or `Err(...)` in any branch; github/board/linear/other all return `Ok(...)`; inner `github_mark_blocked_with` swallows `run_gh` errors (`let _ =` / `Skipped(reason)`). ✅
- **`await_repl_ready` poll/trust logic + `inject_prompt` send sequence** — byte-for-byte unchanged; only `return`→`return true/false` and `Ok(())`→`Ok(ready)`; the `send_bytes → SUBMIT_DELAY → bare Enter` sequence is unchanged context. ✅
- **F2 `compose_issue_body` / `spec_md_from_issue`** — untouched (diff grep empty). ✅
- **F3 `useComposerState.ts` internals** — untouched by `9423b86f` (not in its file list); reused via props only; F1's `initialStartGatedRunProp(modalData)` spread kept at line 195. ✅
- **Cross-slice isolation** — F2 touched no F1 surface; F3 touched no F1/F2 surface (per `--stat`); the only shared file (`NewWorkspaceComposerModal.tsx`, F1+F3) has F3 preserving F1's spread. ✅

---

## 6. Info / non-blocking findings (most-severe first)

1. **`npm run build` is `vite build` (esbuild transpile+bundle), not full `tsc`.** No vite checker plugin exists (`vite.config.*` has none; the script is literally `"build": "vite build"`). The developer's "tsc typecheck green" wording overstates what runs — a pure type error in a spec-008 module that doesn't affect runtime would not fail this gate (it is partially caught only where a passing vitest imports the module at runtime). **Not a defect and not a regression:** the spec's own `verify.sh` defines the UI gate as "vite build + vitest" (both green), and project memory documents bare `tsc` as known-broken for this package (`shared/*` resolution). Flagging only so the reviewer reads "typecheck" as "vite transpile + vitest runtime import," not "full static typecheck."
2. **Desktop `agentum-desktop` cargo crate not built** (needs sherpa dylibs in `target/release`). Consistent with the spec's stated gate (server lib + vite + vitest). The Rust shell wiring of the new UI is therefore not compile-checked here — but F3 added no Rust and F1/F2's Rust is covered by the server lib gate.
3. **The 139 pre-existing vitest failures are real** (missing `@xterm/*` packages, missing `main/ipc/*` modules, unmocked Tauri `api.ui.*`, stale `renderer/src/App.tsx`). Out of scope for spec 008 and disjoint from it, but they mean the UI suite is not a clean "all green" — worth a separate triage ticket eventually. `task_sink::tests` also shows 1 pre-existing `ignored` test (network-gated), unrelated to spec 008.
4. **Scope breadth:** the change spans 25 source/test files across three slices. Expected for a 3-feature spec; each slice is independently gated and cross-slice separation is clean (verified). The harness "32 files modified" scope-warning counts cumulative session file activity, not a code-cohesion problem.

---

## 7. Final verdict & handoff

**VERDICT: PASS-WITH-DEFERRALS.**

Every autonomously-verifiable gate reproduces green with the developer's exact numbers (server 552/0/5, executor 21/0, fmt+clippy clean, live binaries compile `#[ignore]`, vite build green, spec-008 vitest 34/0, full vitest 139-fail baseline confirmed pre-existing). All 11 disclosed deviations are accurate against the code. All sacred surfaces are clean, including the two D5 mechanics (`await_repl_ready`/`inject_prompt` send sequence byte-identical) and the never-`Err` `apply_blocked_transition`. No hollow or missing tests; the D5 live test genuinely asserts the issue→prompt-in-pane→label-flip leg. No defect surfaced.

The only open items are the spec's own by-design deferrals: the **`qa.sh` browser QA gate** (AC 1–5, 11 visual/runtime — requires `AGENTUM_BROWSER_VERIFY`/`browserQaAgentEnabled` armed, else vacuous), the **D5 real-`claude` live tests** (`harness_live_agent.rs` + `harness_start_work_live{,_roles}.rs`, human pre-release step for the sacred #14a change), and the **AC-12 installed-release-app demo** (Mateo). None is a failure.

**Recommendation: hand off to the sdd-reviewer.** Suggested reviewer focus (all pre-verified clean here, worth a second set of eyes): the `inject_prompt` send-sequence byte-identity and the `SettleOutcome`/`await_repl_ready` return-mapping; the `apply_blocked_transition` never-`Err` + five-name remove-set (board can't lie in either direction); F2's assembly-only `intake_grounding_blocks` extraction; F3's `initialStartGatedRunProp` preservation and the seed-gated create-issue prefill. Before release, ensure the three `#[ignore]` live tests are run green (D5) and the `qa.sh` browser pass is executed with the browser-QA knob armed.
