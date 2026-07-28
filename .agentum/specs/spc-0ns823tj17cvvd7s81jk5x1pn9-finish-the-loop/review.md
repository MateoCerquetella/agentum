# Spec 008 — Finish the Loop — Reviewer Sign-off

- **Spec:** `ai/specs/008-finish-the-loop/spec.md` (AC 1–12, D1–D9)
- **Reviewer:** sdd-reviewer (final quality/correctness/maintainability/architecture gate)
- **Date:** 2026-07-03
- **HEAD:** `9d9be973` (worktree `finish-the-loop`, clean tree)
- **Under review:** `51705bf2` (F1 never-silent run path) · `3b6dbd33` (F2 Fast/Complex intake) · `9423b86f` (F3 goal-first workspace) · `9d9be973` (tester artifacts)
- **Method:** independent read of the actual code at HEAD (read-only; no Bash). Gate numbers were re-run by the tester and not re-litigated here — this review verifies judgment: correctness, never-silent integrity, maintainability, architectural consistency, and test quality.

---

## Focus-item verdicts (each with quoted evidence)

### F1 — the never-silent run path (the riskiest slice)

**1. Sacred mechanic `inject_prompt` — send sequence byte-identical → PASS.** `drive.rs:1037-1067`. The only change is the return type and capturing `ready`: `send_bytes → SUBMIT_DELAY → bare Enter` sequence and ordering unchanged; host `?` + target derivation sit exactly where they did; `Ok(ready)`. The three external callers (`sessions.rs:868`, `wiki.rs:329`, `board_goals.rs:852/974`) use `if let Err(e) = inject_prompt(...)`, correctly ignoring the new `Ok(bool)` — no compile break, no behavior change.

**2. Sacred mechanic `await_repl_ready` — poll/trust logic byte-identical → PASS.** `drive.rs:967-1015`. The 0..80 × 700ms poll (~56s), the trust-dialog accept, and the idle-footer match are intact. Only `return` became `return true`/`return false`, with a trailing `false`. Missing-host and remote fixed-delay branches faithfully map the old early-returns. Instrumentation-only (the F-FLAG change).

**3. `wait_for_settle → SettleOutcome` — faithful mapping, four loud sites → PASS.** `drive.rs:1133-1194`. Every old silent `return` maps to a variant: overall-timeout → `TimedOut`, post-grace idle → `Settled`, crash/stop → `Crashed`, bus-closed → `Settled` (no spurious shutdown warning). All four call sites (`:166`, `:364`, `:619`, `:758`) emit `settle_timeout_message(timeout)` **only** on `TimedOut`; `Settled`/`Crashed` fall through to the gate exactly as the old `()` did. The 4th site (`run_role_gate`) is the disclosed deviation, strictly more correct.

**4. D6 `apply_blocked_transition` — `Ok(Skipped)`-never-`Err` → PASS.** `task_sink.rs:771-814`. Every arm returns `Ok(...)`; no `?`, `bail!`, or `Err(...)`. Inside `github_mark_blocked_with` (`:654-679`), `run_gh` errors → `Skipped(reason)`, ensure-label + comment are `let _ = run_gh(...)`; `Applied` is contingent only on the label edit — a failed comment never downgrades. A blocked issue-update can never halt the (already-halted) run.

**5. D6 five-name remove-set — one-of-five holds in both directions → PASS.** Pipeline flip clears blocked: `gh_set_status_label_argv` (`:469-497`) iterates `all_status_label_names(map)` (4 pipeline + blocked) minus target. Blocked flip clears pipeline: `gh_set_blocked_label_argv` (`:503-527`) iterates the 4 pipeline names minus target; the `name != target` dedup never removes the target. Both directions verified — the board can't lie either way.

**6. No shell injection in the D6 comment → PASS.** `gh_issue_comment_argv` (`:532-534`) returns `["issue","comment",number,"--repo",slug,"--body",body]`; `run_gh` (`:585-589`) spawns via `Command::new(program).args(args)` — argv exec, no shell. A multi-line body with backticks/newlines is a single argv token; it cannot inject a flag.

**7. `record_feature_failure → (bool, u32)` → PASS.** `harness.rs:359-400`. `attempts = feature.attempts` after the increment; `handle_gate_failure` destructures `let (blocked, attempts) = ...` (`drive.rs:305`) and threads the true capped count into the comment.

**8. `gh_in_dir` honors `AGENTUM_GH_BIN` (local arm only) → PASS.** `git_fs.rs:92-97`: `HostKind::Local` reads `AGENTUM_GH_BIN` (default `"gh"`); the `Ssh` arm still hardcodes `gh`. Var unset ⇒ exactly `gh` ⇒ production byte-identical. Mirrors `task_sink::gh_bin()`.

**9. UI never-silent (armed `!repoId` + server-error detail + events bridge) → PASS.** `useComposerState.ts:2340-2370`: the guard's `if` conditions map one-to-one onto `firstStartGatedRunBlocker`; the toast fires only when armed, so the non-armed bare-`return` is byte-identical; the #226 `repoId:''` edge is covered. `:2329-2334`: the catch surfaces `error.message.trim()` (the server's `— {detail}`). `harness-client.ts:378-403`: `subscribeHarnessRunErrors` fires once (`fired` guard), then `dispose()`s (clears timer + closes stream), self-closes after `windowMs` (120s). No socket leak.

**10. No residual silent path on the start route → PASS.** `routes/harness.rs:508-633`. `start_work` returns exactly two 200 shapes: `already_running` (always with `harness_id`) → `toast.info`, and `run_started` (always with a real `harness_id` + a spawned drive task) → client subscribes to run errors. Every failure is `Err(ApiError)` → non-2xx → `catch` toast. No 200 produces no client-visible signal.

**11. Live start-work-leg test genuinely asserts the leg → PASS (real, not hollow).** `tests/support_start_work/mod.rs`. `MARKER` is embedded in the canned issue's first acceptance box (`:102`) → planned feature → injected prompt → pane; `capture_pane_has_marker` reads full scrollback (`capture-pane -p -S -`) so it proves the **prompt landed** (AC 2). Also asserts `runStarted==true`, a spawn signal, `--add-label status/todo` (AC 3 plan flip), and `--add-label status/in-progress` for roles-off. claude+tmux real, only the `gh` fetch stubbed. Two `#[ignore]` binaries — the D5 human pre-release gate.

### F2 — Fast / Complex intake

**12. `intake_grounding_blocks` assembly-only; Fast byte-identical → PASS.** `chat.rs:308-356` extracts the tuple; `interviewer_instructions` (`:367-401`) unpacks it and runs the same `format!`. Guarded by `build_intake_instructions_fast_equals_interviewer_verbatim` (stages `[0,1,3,5,9,250]` + no-grounding) **and** the two pre-existing content tests. Byte-identity by construction + content pin.

**13. Five single-topic passes, reflect-back, stage-5 convergence → PASS.** `socratic_pass_body` (`:473-502`): WHO/WHAT/WHY/DONE-CRITERIA/RISKS, reflect-back on 2–5, stage 5 "STOP asking questions" + "Preview issues". `socratic_stage_instructions` clamps `stage.clamp(1,5)` — no off-by-one. Pinned by `socratic_stage_prompts_cover_one_pass_each_and_converge_at_five` + `socratic_stage_clamps_out_of_range`.

**14. `chat_auth_gate` — Complex has no bypassing endpoint → PASS.** `chat.rs:511-513` (`None ⇒ BadRequest(NO_CREDS_MSG)`), called first in both handlers (`:552` `chat`, `:654` `chat_stream`) before any mode/stage prompt build. No separate Complex route; `NO_CREDS_MSG` surfaces on Complex's first turn by construction. Pinned hermetically (`:2504`).

**15. Client reducer + store wiring (one pass/turn, Fast fast) → PASS.** `socratic-intake.ts`: `advanceIntake` unchanged for Fast, `clampStage(stage+1)` for socratic; `normalizeIntake` restarts a cleared/legacy store cleanly (D1). `chat-store.ts:184-202` persists `advanceIntake(intakeNow)` in both branches — exactly one advance per send. `ChatPage.tsx:318-322`: Enter → Fast; a continuing thread inherits its stored mode (Enter never derails an in-progress Complex interview). `ChatRequest` fields `#[serde(default)]` → Fast/old-client wire unchanged.

### F3 — goal-first workspace

**16. `workspace-goal-step.ts` pure logic → PASS.** All helpers pure (no React/DOM/xterm). Required set = goal + repoId, blocker never silent, exactly three skippable steps each naming its reused primitive.

**17. F1 preservation — `initialStartGatedRunProp` spread intact + `useComposerState` untouched → PASS.** `shouldStartAtGoalStep` returns `false` when `startGatedRun` is set, so an F1 Tasks-hop open goes straight to `'details'` — the goal step never fronts it. `...initialStartGatedRunProp(modalData)` is spread at `NewWorkspaceComposerModal.tsx:195` inside `useComposerState({...})`, uncloberrable, resolving to `{}` on a goal-path open. `useComposerState` called with props only. The seed-gated create-issue prefill is a one-shot `useRef`+`useEffect` on the **public** `onCreateIssueTitleChange`/`onCreateIssueBodyChange`; "Skip to details" (`seed=null`) is byte-identical to today.

**18. Goal step imports zero composer internals → PASS.** `NewWorkspaceGoalStep.tsx:1-11` imports only React, lucide-react, `ui/*`, `RepoCombobox`, the pure `lib/workspace-goal-step`, and the `Repo` type.

### Architectural consistency

- **One launch path** — no new spawn; all agents stay on `spawn_agent_into_pane`. ✓
- **YOLO marker push** — untouched in `drive.rs`. ✓
- **Best-effort tracker** — `apply_blocked_transition` never `Err`; `drive.rs:334-344` logs Ok and Err non-fatally. ✓
- **D1 no new store tables** — `chat-history.ts:48` adds `intake?: IntakeState` as a per-machine localStorage field. ✓
- **D-A `TrackerPhase` stays four variants** — `status/blocked` is a fixed GitHub-only label + sibling seam. ✓
- **No new `is_public` auth holes** — `auth.rs:74-98` allowlist unchanged (health/cert/auth/hook); chat + start-work stay behind the bearer-token middleware, `chat_auth_gate` layered as an additional Anthropic-credential gate. ✓
- **D5** — the two sacred-mechanic changes are behavior-preserving and gated on the live tests. ✓

---

## Blockers

**None.**

---

## Should-fix (non-blocking; fold into a follow-up)

1. **UI gate does not run a full static typecheck.** `npm run build` is `vite build` (esbuild transpile+bundle) with no checker plugin; the "tsc typecheck green" wording in the dev/tester notes overstates what runs. **Not a spec-008 defect** (the spec's `verify.sh` defines the UI gate as vite build + vitest, both green; bare `tsc` is documented-broken for this package via `shared/*` resolution) — but adding real typechecking to the UI gate (a vite checker plugin or a working `tsc --noEmit` project ref) is worth a **project-wide follow-up ticket**, bundled with the vitest-baseline triage. Concurs with tester Info #1; elevated to a should-fix CI recommendation, not a send-back.

---

## Nits (trivial; no action required to ship)

1. **`blocked_comment_body` fence robustness.** `task_sink.rs:538-549` wraps `gate_tail` in a triple-backtick fence; a gate tail containing a triple-backtick would break the markdown fence. Cosmetic only (single argv token, no injection; architecture-prescribed format). A uniquified fence or backtick-stripping would harden it later.
2. **`apply_blocked_transition` returns `Result` but never `Err`.** Deliberate signature-parity with `apply_tracker_transition` (documented); the `Err(e)` arm at `drive.rs:339` is dead but harmless/defensive. Leave as-is.
3. **`SettleOutcome::Crashed` constructed but no caller special-cases it** (all four sites check only `== TimedOut`). Falls through to the gate as the old `()` did; descriptive/future-proof, clippy-clean. No change needed.

---

## Call on the tester's 4 Info findings

- **#1 (vite vs tsc):** Concur; non-blocking; elevated to a should-fix CI follow-up (above).
- **#2 (desktop cargo crate not built):** Concur — non-blocking. F3 added no Rust; no new Tauri commands.
- **#3 (139 pre-existing vitest failures):** Concur — non-blocking. Corroborated pre-existing and disjoint; separate triage ticket.
- **#4 (scope breadth):** Concur — non-blocking. Cross-slice separation verified clean.

---

## VERDICT

**SIGN-OFF → SHIP-READY (0 blockers).**

The implementation is correct, never-silent by construction, maintainable (comments state *why*; the pure extractions are genuinely DOM/xterm-free and unit-pinned), and architecturally consistent (one launch path, YOLO untouched, best-effort tracker, D1–D9 honored, no new auth holes, no new store tables). The two D5 sacred mechanics are behavior-preserving; the D6 blocked escalation is never-`Err` with a structurally-honest five-name remove-set; F2's Fast path is byte-identical; F3 preserves F1's Tasks hop. The live start-work-leg test genuinely asserts issue → route → session → prompt-in-pane → label-flip.

**Sign-off = the implementation is ready, NOT that it is released.** The following remain **HUMAN pre-release steps**, per standing convention and D5:
- **Release promotion** develop → staging (QA) → main (release, tag `vX.Y.Z`).
- **D5 live tests green** (real `claude`): `harness_live_agent.rs`, `harness_start_work_live.rs`, `harness_start_work_live_roles.rs` — the merge gate for the sacred #14a change; they cannot run in the autonomous gate.
- **`qa.sh` browser pass** with the browser-QA knob armed (`AGENTUM_BROWSER_VERIFY` / `browserQaAgentEnabled`) — else it passes vacuously.
- **AC-12 installed-release-app demo** (Mateo): goal-first → Complex chat → issues → one click → green gate with live label flips, evidenced by the issue's label-flip + harness-comment trail and an `ai/STATE.md` decision-log line.
