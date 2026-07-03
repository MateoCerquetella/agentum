# Spec 008 — Developer tasks (F1 ONLY: the never-silent run path)

- **Spec:** 008-finish-the-loop
- **Feature:** **F1** — the run actually runs (the spine). AC 1–4.
- **Role:** Developer (sdd-developer)
- **Date:** 2026-07-03
- **Base:** worktree `finish-the-loop` (tip `0e6812f8`)

> **Scope guardrail:** this iteration implements **F1 only**. **F2** (Chat
> Fast/Complex intake) and **F3** (goal-first workspace creation) are **deferred
> to later, separate developer iterations** — no F2/F3 code was written.
> `interviewer_instructions` / `compose_issue_body` / `ChatRequest` (F2) and the
> `useComposerState` creation engine / goal-step component (F3) are untouched.

F1 was built in the architecture's ordered sequence (§F, cheapest/safest first,
sacred last): **Step 1 → Step 2 → Step 4 → Step 5 → Step 3**.

---

## The four real silences F1 closes (architecture §B.1)

| # | Silence (the "dead click / silent hang") | Closed by |
|---|---|---|
| **#15** | `wait_for_settle` timed out after up to **1800 s** returning `()` — gate ran on an unchanged tree, zero events | **Step 1** |
| **#16** | A blocked gate escalated only in-app — the **issue** stayed on `status/in-progress` with no comment | **Step 2** |
| **#14a** | `await_repl_ready` fell through returning `()` — the prompt fired **blind**, no event/error | **Step 3** (sacred) |
| **#2** | The composer's armed `!repoId` guard `return`ed silently (the #226 chat-origin `repoId:''` edge) | **Step 4** |
| (+#5) | The start-failure toast hid the server's actionable `ApiError` detail | **Step 4** |

---

## Step 1 — `wait_for_settle → SettleOutcome` (#15, AC 2). NOT sacred.

**Root cause:** `wait_for_settle` returned `()` on timeout; the caller couldn't
tell a real settle from an up-to-1800 s hang, so it gated in silence.

**Landed:**
- `crates/agentum-server/src/harness/drive.rs`
  - New `pub(crate) enum SettleOutcome { Settled, Crashed, TimedOut }`.
  - `wait_for_settle(...) -> SettleOutcome` — every `return` mapped to a variant;
    the timeout paths return `TimedOut`, an idle/early-idle returns `Settled`, a
    `session.crashed`/`session.stopped` returns `Crashed`, a closed bus returns
    `Settled` (no spurious warning on shutdown). Loop/timing logic unchanged.
  - New `settle_timeout_message(timeout)` builder for the loud log.
  - Loud `engine.log(..., settle_timeout_message(timeout))` on `TimedOut` at
    **all 4** call sites: `drive_inner` feature loop, `handle_gate_failure`
    retry, `run_qa_agent_gate`, and `run_role_gate` (the 4th uses `None`
    feature id). *Deviation from the "3 call sites" wording — see Deviations.*
- `crates/agentum-server/src/harness.rs` (settle unit tests, ~:1316):
  - `settle_returns_after_grace_on_early_idle` now asserts `== Settled`.
  - `settle_ignores_events_for_other_sessions` now asserts `== TimedOut`.
  - **New** `settle_returns_crashed_on_session_stop` asserts `== Crashed`.
  - Import widened: `use super::drive::{… SettleOutcome};`.

---

## Step 2 — D6 `status/blocked` escalation (#16, AC 4). Localized to `task_sink.rs`.

**Root cause:** a blocked feature was loud in-app but the GitHub issue kept
`status/in-progress` and got no explanation — the board lied.

**Landed (`crates/agentum-server/src/task_sink.rs`):**
- `const GITHUB_BLOCKED_LABEL: (&str,&str) = ("status/blocked","b60205")` — a
  **fixed** GitHub-only label; **`TrackerPhase` stays four variants** (D-A).
- `all_status_label_names(map) -> Vec<&str>` — the 4 configured pipeline names +
  blocked (the one-of-five remove-set).
- `gh_set_blocked_label_argv(number, slug, map)` — add `status/blocked`, remove
  the four pipeline names.
- `gh_issue_comment_argv(number, slug, body) -> [&str;7]` — pure; body is one
  argv token (never shell-interpolated).
- `blocked_comment_body(feature, gate_label, attempts, gate_tail)` — the AC-4
  comment: retry count + a collapsible `<details>` fenced gate tail.
- `github_mark_blocked_with(program, slug, number, feature, gate_label, attempts,
  gate_tail, map)` — ensure-create → **one** edit (add blocked + remove 4) → one
  best-effort comment; `Applied` **iff the label edit succeeds** (comment failure
  never downgrades).
- `pub async fn apply_blocked_transition(store, provider, tracker_id,
  tracker_url, feature, gate_label, attempts, gate_tail) -> Result<TransitionResult>`
  — GitHub does the above; **board/linear → `Skipped("no blocked state")`**;
  **never `Err`** for a tracker hiccup (best-effort contract). `store`/`tracker_id`
  kept for signature-parity with `apply_tracker_transition`.
- **The one-of-five invariant made structural:** `gh_set_status_label_argv`'s
  remove loop widened from `map.labels()` to `all_status_label_names(map)` minus
  target — so **every pipeline transition also clears `status/blocked`** (a
  re-driven blocked feature drops the label at InProgress; board honest both ways).

**Wired (`crates/agentum-server/src/harness/drive.rs`, `handle_gate_failure`):**
- After `engine.set_state(Blocked)`, call `apply_blocked_transition(...)` threading
  `feature.tracker_provider`/`tracker_url`/`feature.name`/`gate_label`/`attempts`/
  `tail(output, 2000)`; log Ok **and** Err non-fatally.
- `attempts` comes from **widening `record_feature_failure`** (`harness.rs`) to
  return `(bool /*blocked*/, u32 /*attempts*/)` (the return-widen the architecture
  offered — cleaner than re-reading config in `handle_gate_failure`, which has no
  `config` in scope). Its two result-binding tests updated to destructure + assert
  the attempts; the two statement-form callers (`run_verify`, the reset test loop)
  discard the tuple unchanged.

**Tests (`task_sink.rs`):**
- Renamed `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three`
  → `…_adds_one_removes_the_three_pipeline_and_blocked` (expected argv now carries
  the trailing `--remove-label status/blocked`; asserts 4 removes incl. blocked).
- Arity-updated `gh_set_status_label_argv_uses_configured_names` and
  `…_never_removes_the_target_on_name_collision` (blocked appended to removes).
- Updated `github_transition_with_custom_map_flips_configured_names` (edit line
  gains `--remove-label status/blocked`).
- **New:** `gh_set_blocked_label_argv_adds_blocked_removes_four_pipeline`,
  `gh_issue_comment_argv_body_is_a_single_token`,
  `blocked_comment_body_carries_attempts_and_gate_tail`,
  `github_mark_blocked_with_fake_gh` (`#[cfg(unix)]`, newline-safe fake-gh:
  logs single-line argv, dumps the multi-line comment body to a file, asserts
  ensure + edit + comment), `apply_blocked_transition_board_and_linear_are_skipped`,
  `apply_blocked_transition_github_without_url_is_skipped` (hermetic).

---

## Step 4 — UI toasts + events bridge (#2/#5, AC 1). No composer-engine restructure.

**Root causes:** (#2) the armed `!repoId` guard returned silently — where the
#226 chat-origin `repoId:''` issue lands; (#5) the start-failure toast hid the
server's actionable detail; (§B.5) a composer-started run navigates away, so its
drive-phase failure had no watcher.

**Landed:**
- **New pure `crates/agentum-desktop/ui/src/lib/start-gated-run-precondition.ts`**
  — `firstStartGatedRunBlocker(state)` names the first unmet precondition (in the
  guard's `||` order) or `null`. Pure (no toast/DOM/xterm).
- **New pure `crates/agentum-desktop/ui/src/lib/composer-modal-props.ts`** —
  `initialStartGatedRunProp(modalData)` extracts the modal's inline
  `startGatedRun → initialStartGatedRun` spread so it is unit-pinnable.
- `crates/agentum-desktop/ui/src/hooks/useComposerState.ts`:
  - `submit` guard: when `startGatedRun` is armed and a precondition trips, it
    now `toast.error(firstStartGatedRunBlocker(...))` instead of a bare `return`.
  - `maybeStartGatedRun`: the catch toast now appends the server `ApiError`
    detail (`error.message` — `request()` already appends `— {detail}`); and on a
    successful start it `subscribeHarnessRunErrors(result.harnessId, …)` to toast
    the first drive-phase error.
- `crates/agentum-desktop/ui/src/runtime/harness-client.ts`:
  - New `subscribeHarnessRunErrors(harnessId, onError, windowMs=120_000)` — reuses
    `openHarnessEventStream`, fires once on the first `error` for that run, then
    self-closes (also after `windowMs`, so a healthy run never holds the socket).
- `crates/agentum-desktop/ui/src/components/NewWorkspaceComposerModal.tsx`:
  - The inline `…startGatedRun ? {…} : {}` replaced with
    `…initialStartGatedRunProp(modalData)` (+ import). *Minimal F1-test-enabling
    extraction in a file F3 will also touch — see Deviations.*

**Tests (vitest):**
- **New** `src/lib/composer-modal-props.test.ts` (2) — `modalData.startGatedRun →
  initialStartGatedRun` armed/empty.
- **New** `src/lib/start-gated-run-precondition.test.ts` (4) — the armed `!repoId`
  guard toasts (incl. the `''` #226 edge), every other unmet precondition names a
  blocker, guard-order.
- `src/lib/issue-side-effect-gate.test.ts` — **new** case: **every**
  `IssueSideEffectSkipReason` yields a distinct, non-empty toast on the
  start-gated-run route (no silent branch).

---

## Step 5 — the new live test (the merge gate for Step 3). `#[ignore]`.

**Root cause:** `harness_live_agent.rs` starts from `engine.start` + `drive`, so
it **skips** the leg issue → `POST /api/harness/start-work` → session opens →
prompt lands. The new test covers exactly that leg.

**Landed:**
- `crates/agentum-server/tests/support_start_work/mod.rs` — the shared driver
  `run(roles_on)`: stages an empty project, stubs the GitHub **fetch** via a fake
  `gh` (`AGENTUM_GH_BIN`, a canned issue with two `- [ ]` boxes + a distinctive
  `MARKER`), points `AGENTUM_GITHUB_CONFIG` at an absent file, boots
  `serve_embedded_loopback_state`, sets `harness.sdd.roles.enabled`, `POST`s
  start-work over the loopback (`reqwest`), asserts `runStarted` + `harnessId`,
  then watches for the spawn signal + the `MARKER` in a harness pane
  (`tmux capture-pane -p -S -`) + the fake-gh `status/todo` (and, roles-off,
  `status/in-progress`) label edits. **claude + tmux stay real.**
- `crates/agentum-server/tests/harness_start_work_live.rs` — `#[ignore]` roles-OFF
  primary (first spawn = the FEATURE agent; asserts `AgentSpawned`), `exit(0)`.
- `crates/agentum-server/tests/harness_start_work_live_roles.rs` — `#[ignore]`
  roles-ON companion (first spawn = the PM role gate; accepts the "spawning pm
  agent" `Log` as the session-open signal), `exit(0)`.

Two binaries (shared `#[path]` module), not one binary with two tests: each owns
its `std::process::exit(0)` (the model `harness_live_agent.rs` needs it to dodge a
runtime-teardown hang; a single `exit(0)` would kill a sibling test in-process).

**Enabling seam:** `crates/agentum-server/src/host_runtime/git_fs.rs::gh_in_dir`
local arm now honors `AGENTUM_GH_BIN` (defaults to `"gh"`) so the fetch is
stubbable — parallel to `task_sink::gh_bin()`; production byte-identical.

---

## Step 3 — the sacred readiness-bool (#14a, AC 2). DONE LAST. Behavior-preserving.

**Root cause:** `await_repl_ready` fell through returning `()` when it never saw
the idle footer; `inject_prompt` then fired the prompt blind — the deepest silence.

**Landed (`crates/agentum-server/src/harness/drive.rs`):**
- `await_repl_ready(...) -> bool` — `true` iff the idle footer was seen; `false`
  for a remote fixed-delay fallback, a missing host, or the ~56 s poll expiring.
  **The poll / trust-accept / fixed-delay logic is byte-for-byte unchanged** —
  only `return` → `return true`/`return false` and a trailing `false`.
- `inject_prompt(...) -> anyhow::Result<bool>` — bubbles the readiness bool.
  **The send sequence (`send_bytes` → `SUBMIT_DELAY` → bare Enter) is byte-for-byte
  unchanged**; only the return type + capturing `ready` changed.
- New `repl_not_ready_message()` (the exact §B.1 #14a copy); emitted at the 4
  drive call sites when `!ready` (`drive_inner`, `handle_gate_failure` retry,
  `run_qa_agent_gate`, `run_role_gate`).
- The `board_goals.rs` / `sessions.rs` / `wiki.rs` callers use
  `if let Err(e) = inject_prompt(...)` — they ignore the new `Ok(bool)`
  unchanged (no edits needed).

> **D5 MERGE GATE (human pre-release step).** This is the ONE F1 change that
> touches a sacred autonomy mechanic. It is behavior-preserving (instrumentation
> only), but per D5 it may only ship once **BOTH** live tests are green:
> `tests/harness_live_agent.rs` **and** the new `tests/harness_start_work_live.rs`
> (+ its roles-ON companion). Those spawn a **real** `claude` agent and **cannot
> run in the autonomous/CI gate** — so **running them green is a HUMAN
> pre-release step**, like the staging browser-QA pass. The autonomous gate below
> does NOT and cannot exercise them.

---

## Build + test results (observed)

Cargo lives at `~/.cargo/bin` (not on the default PATH).

| Gate | Command | Result |
|---|---|---|
| Backend lib | `cargo test -p agentum-server -p agentum-executor --lib` | **agentum-server 546 passed / 0 failed / 5 ignored; agentum-executor 21 passed / 0 failed.** Includes the F1 additions: +7 new server unit tests (1 settle-crash + 6 D6) and the renamed/updated D6 + settle + `record_feature_failure` tests. |
| Format | `cargo fmt --all` then `cargo fmt --all --check` | **clean (exit 0).** |
| Lints | `cargo clippy -p agentum-server --tests` | **0 warnings** (added `#[allow(clippy::too_many_arguments)]` to the two 8-arg D6 fns — architecture-prescribed signatures, matching `handle_gate_failure`; reworded a doc line to drop a `+`-bullet). |
| Live tests compile | `cargo test -p agentum-server --test harness_start_work_live --test harness_start_work_live_roles --test harness_live_agent --no-run` | **all 3 binaries compile.** (`#[ignore]` — correctly NOT run in the gate.) |
| UI build | `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix crates/agentum-desktop/ui` | **built in ~1m18s (vite, typecheck green).** |
| UI vitest | `npx vitest run src/lib/{composer-modal-props,start-gated-run-precondition,issue-side-effect-gate}.test.ts` | **14 passed / 0 failed** (3 files; +7 new F1 assertions). No existing test imports the changed hook/runtime/modal, so no regression surface. |

*The desktop **cargo** crate was NOT built (it needs the sherpa dylibs in
`target/release`); per the task the gate is the server lib tests + vite build +
vitest, all green.*

---

## Deviations from the architecture (with rationale)

1. **Loud settle-timeout log at 4 call sites, not 3.** The task named the 3
   feature-scoped sites (`drive_inner`, `handle_gate_failure`, `run_qa_agent_gate`);
   `wait_for_settle` also has a 4th caller in `run_role_gate`. Since the return
   type changed, every caller must consume it — I added the loud log there too
   (with `None` feature id), which is strictly more correct and matches
   architecture §1's "instrumentation uniform across spawn paths".
2. **`gh_in_dir` honors `AGENTUM_GH_BIN` (git_fs.rs).** Not on F1's "may touch"
   list, but the task explicitly directs stubbing the live-test fetch via
   `AGENTUM_GH_BIN`, and the fetch path (`fetch_github_issue → gh_in_dir`)
   hard-coded `"gh"`. The one-line change (local arm only, defaults to `"gh"`)
   mirrors the existing `task_sink::gh_bin()` seam; production is byte-identical.
3. **Minimal extraction in `NewWorkspaceComposerModal.tsx`.** F3 owns the modal
   restructure, but the F1 test "`modalData.startGatedRun → initialStartGatedRun`"
   needs a testable seam. I extracted the existing inline one-liner into the pure
   `initialStartGatedRunProp` and wired it in — behavior-identical, orthogonal to
   F3's goal-step work (which fronts the modal), no conflict expected.
4. **`record_feature_failure` return-widen** (`harness.rs`) chosen over threading
   `max_retries` into `handle_gate_failure` — the architecture offered both; the
   return-widen avoids re-reading config where `config` isn't in scope.

## Protected invariants (confirmed untouched)

- **One launch path** — no new spawn; `spawn_agent_into_pane` untouched.
- **YOLO marker push** in drive.rs — untouched.
- **`apply_blocked_transition`** is `Ok(Skipped)`-never-`Err` (best-effort tracker).
- **`inject_prompt` send sequence** and **`await_repl_ready` poll/trust logic** —
  byte-for-byte unchanged (only return types changed).
- **F2/F3 surfaces** — `ChatRequest`/`interviewer_instructions`/`compose_issue_body`
  and the `useComposerState` creation engine — untouched.

## Handoff

Ready for **sdd-tester / sdd-reviewer**. The autonomous gate (backend lib +
fmt + clippy + vite + vitest) is green above. The **two `#[ignore]` live tests are
the human pre-release merge gate** for the Step-3 sacred change (they spawn a real
claude agent and cannot run autonomously) — run them with:

```
cargo test -p agentum-server --test harness_live_agent          -- --ignored --nocapture
cargo test -p agentum-server --test harness_start_work_live       -- --ignored --nocapture
cargo test -p agentum-server --test harness_start_work_live_roles -- --ignored --nocapture
```

---

# F2 — Fast / Complex intake (Chat embeds real SDD intake). AC 5–8.

- **Feature:** **F2** — the Chat composer's Fast/Complex entry buttons + a staged
  Socratic interview. AC 5–8.
- **Role:** Developer (sdd-developer)
- **Date:** 2026-07-03
- **Base:** worktree `finish-the-loop` (tip `51705bf2`, F1 already landed)

> **Scope guardrail:** this iteration implements **F2 only**. F1 was already
> landed (above) and **NONE of its surfaces were touched** (`drive.rs`,
> `harness.rs`, `task_sink.rs`, `git_fs.rs`, `useComposerState.ts`,
> `harness-client.ts`, the composer UI). **F3** (goal-first workspace) is a later
> iteration — `NewWorkspaceComposerModal`/`NewWorkspaceGoalStep`/`useComposerState`
> internals are untouched. Built to architecture §C (D-B explicit `{mode,stage}`;
> D1 stateless server + client-owned stage; D2 no forced thinking; D4 no sticky
> preference; D8 both modes converge on the unchanged `compose_issue_body`).

## The seam (architecture §C): an explicit `{mode, stage}` on `ChatRequest`

Fast stays byte-identical; Socratic is five server-owned per-stage prompts that
reuse the SAME grounding blocks; both converge on the existing "Preview issues" →
`compose_issue_body` path. The server owns NO stage state — it's a pure
`(mode, stage) → system prompt` function; the CLIENT advances the stage.

## Server — `crates/agentum-server/src/routes/chat.rs` (AC 5–8)

**Landed:**
- **`enum IntakeMode { Fast, Socratic }`** — `#[derive(Deserialize, Clone, Copy,
  PartialEq, Eq, Debug)] #[serde(rename_all = "snake_case")]` (wire = `"fast"` /
  `"socratic"`). Deliberately NOT `Debug`-deriving `Auth` (it wraps a secret).
- **`ChatRequest` gains two `#[serde(default)]` fields:** `mode: Option<IntakeMode>`
  (None ⇒ Fast) and `stage: Option<u8>` (1..=5, clamped server-side). Old clients
  and the Fast path stay byte-identical on the wire (AC 6).
- **`intake_grounding_blocks(workdir, repo_slug, repo_context, wiki_context) ->
  (String, String, &'static str, String)`** — the ctx / repo_block / access_rule /
  wiki_block extracted VERBATIM from `interviewer_instructions` so Fast and every
  Socratic pass ground identically. `interviewer_instructions` now calls it and
  assembles the SAME format string → **output byte-for-byte unchanged** (the two
  pre-existing interviewer tests still pass, plus the new byte-identical pin).
- **`build_intake_instructions(mode, stage, …) -> String`** — the router:
  `Fast => interviewer_instructions(…)` VERBATIM; `Socratic =>
  socratic_stage_instructions(stage, …)`. Both `chat` and `chat_stream` now route
  through it (was a direct `interviewer_instructions` call).
- **`socratic_stage_instructions(stage, …)`** — reuses `intake_grounding_blocks`,
  clamps `stage` into 1..=5, swaps the job/Rules body for a single-topic pass via
  `socratic_pass_body(stage)`. Frame + Rules shared across passes; the pass is the
  one thing the turn does.
- **`socratic_pass_body(stage) -> &'static str`** — the five single-topic passes
  (AC 7): 1 WHO (nothing to reflect yet), 2 WHAT (reflect WHO), 3 WHY (reflect
  WHAT), 4 DONE CRITERIA (reflect WHY, checkbox-shaped), 5 RISKS & SCOPE (reflect
  criteria, then STOP asking + point at **"Preview issues"** — the same
  convergence Fast uses, D8).
- **`chat_auth_gate(auth: Option<Auth>) -> Result<Auth, ApiError>`** — the single
  credential gate BOTH intake handlers now run FIRST, before any mode/stage prompt
  is built. `None ⇒ ApiError::BadRequest(NO_CREDS_MSG)`. Complex rides the SAME
  gate (no separate endpoint) so `NO_CREDS_MSG` surfaces on its first turn by
  construction (AC risk). Also unified `chat`'s previously-inline (byte-identical)
  no-creds literal onto `NO_CREDS_MSG`.
- **Handlers wired:** `chat` + `chat_stream` parse `mode = body.mode.unwrap_or(Fast)`,
  `stage = body.stage.unwrap_or(1)`, and call `build_intake_instructions`. Model/
  config identical for both modes; the existing `thinking` opt-in still applies
  (D2 — no forced thinking).

**Server stays stateless (D1):** no store tables, no session state — the stage
travels in each request.

**Tests (`chat.rs` `mod tests`, +6):**
- `build_intake_instructions_fast_equals_interviewer_verbatim` (**AC 6**) — Fast ==
  `interviewer_instructions` across a spread of stages (incl. junk) + a no-grounding
  case; the byte-identical pin guarding a future fold-Fast-into-Socratic refactor.
- `socratic_stage_prompts_cover_one_pass_each_and_converge_at_five` (**AC 7**) —
  per-stage: each names its one pass topic + (stages 2–5) the reflect-back
  instruction; only stage 5 names "Preview issues" + "STOP asking questions".
- `socratic_stage_clamps_out_of_range` — 0 ⇒ pass 1; 9/250 ⇒ pass 5.
- `socratic_stage_reuses_the_shared_grounding_blocks` — repo block + grounded
  access rule + wiki when present; honest-blind rule when absent.
- `chat_request_defaults_and_decodes_intake_mode` — serde-default (old client ⇒
  None) + snake_case decode of `"fast"`/`"socratic"`.
- `chat_auth_gate_surfaces_no_creds_when_unauthed` (**AC risk**) — unauthed ⇒
  `BadRequest(NO_CREDS_MSG)`; authed (ApiKey/Oauth) ⇒ pass-through. See Deviation 1
  for why this pins the shared gate rather than a live `chat_stream` call.

## Client — the pure reducer + composer wiring (AC 5, 7)

**Landed:**
- **New pure `crates/agentum-desktop/ui/src/lib/socratic-intake.ts`** — the
  client-owned state machine (no React/DOM/xterm): `IntakeMode`, `IntakeState`,
  `clampStage`, `fastIntake`, `socraticIntake`, `advanceIntake` (one pass/turn,
  cap 5, Fast never advances — the **AC 7 progression invariant**), `isSocraticComplete`,
  `normalizeIntake` (absent/legacy ⇒ Fast; clamps a bad stage — the cleared-store
  clean-restart, D1).
- `crates/agentum-desktop/ui/src/runtime/chat-history.ts` — `Conversation` gains
  `intake?: IntakeState` (persisted on the existing localStorage record — D1, **no
  new store table**; absent on pre-008 threads ⇒ Fast). Tolerant loader unchanged
  (extra field is a no-op; no migration).
- `crates/agentum-desktop/ui/src/runtime/chat-client.ts` — `streamChat` (and
  `sendChat`) opts gain `mode?`/`stage?`, threaded into the POST body (both
  serde-default, so the Fast/old-client wire is byte-identical).
- `crates/agentum-desktop/ui/src/runtime/chat-store.ts` — `sendChatMessage` takes
  `mode?`; computes `intakeNow` (a continuing thread INHERITS its stored intake via
  `normalizeIntake`; a new thread starts at `{mode ?? 'fast', stage: 1}`), sends
  `intakeNow` to `streamChat`, and persists `advanceIntake(intakeNow)` on the
  conversation so the NEXT turn runs the next pass.
- `crates/agentum-desktop/ui/src/components/harness/ChatPage.tsx` — **two entry
  buttons** (**AC 5**): **Fast feature** (`Zap`, `submitWith('fast')`) and
  **Complex feature** (`Brain`, `submitWith('socratic')`), shown on a NEW chat;
  a "Complex feature · pass N of 5" indicator on a continuing socratic thread.
  `submit` (Enter/arrow) is the Fast default ("Fast must stay fast" — Enter never
  triggers the five-pass interview); a continuing thread keeps its stored mode in
  the store. Per-feature choice, **no sticky preference** (D4).

**Tests (`socratic-intake.test.ts`, +5):** the AC-7 progression reducer — advances
exactly one pass per user turn `[1,2,3,4,5,5,5]`, caps at 5 + `isSocraticComplete`,
Fast never advances, `clampStage`, `normalizeIntake` legacy/absent ⇒ Fast.

## Convergence (AC 7/8) — unchanged, by construction

After stage 5 the client stops advancing and requests the **same** Preview-issues
draft as Fast — `compose_issue_body` / `spec_md_from_issue` are **untouched** (D8).
Both modes therefore end at identical SDD-shaped issue bodies. The stage-5 prompt
points the user at the existing "Preview issues" button (already shown once
`hasAssistantReply`); no new convergence surface.

## Build + test results (observed)

| Gate | Command | Result |
|---|---|---|
| Backend lib | `cargo test -p agentum-server --lib` | **552 passed / 0 failed / 5 ignored.** F1's 546 stayed green; +6 F2 chat tests. |
| Format | `cargo fmt --all` then `--check` | **clean (exit 0).** |
| Lints | `cargo clippy -p agentum-server --tests` | **0 warnings / 0 errors.** |
| UI build | `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix …/ui` | **built in ~1m23s (vite, tsc typecheck green).** |
| UI vitest (new) | `npx vitest run src/lib/socratic-intake.test.ts src/runtime/chat-client.test.ts` | **10 passed / 0 failed** (5 new reducer + 5 existing chat-client). |
| UI vitest (full, diligence) | `npx vitest run` | **5746 passed / 139 failed (43 files).** The 139 failures are a **pre-existing baseline** — a base-commit (`51705bf2`, F1, no F2 changes) run shows the **identical 139 failed / 43 files**; my changes added **+5 passing, 0 new failures**. Failures are in unrelated domains (sidebar color-class drift, git-status, settings, tab-bar, editor, unmocked Tauri `invoke`); only 2 test files import my modules and **both pass**. |

*The desktop **cargo** crate was NOT built (sherpa dylibs); per the task the gate
is the server lib tests + vite build + vitest, all green.*

## Deviations from architecture §C (with rationale)

1. **The no-creds pin tests the shared `chat_auth_gate` helper, not a live
   `chat_stream` no-creds call.** Architecture named
   `chat_stream_returns_no_creds_when_unauthed`. On macOS (this dev machine)
   `resolve_auth()` falls back to the Claude **Keychain**
   (`usage.rs::read_macos_keychain_cred`, `security find-generic-password`), so a
   machine with `claude` installed **cannot be forced to "no creds" via env** — a
   live-handler no-creds test would be non-hermetic (and would proceed to a real
   Anthropic call). I extracted the gate BOTH handlers run first into
   `chat_auth_gate(Option<Auth>)`, wired it in, and pin it hermetically with an
   explicit `None`/`Some`. This is the honest hermetic equivalent and guards the
   REAL invariant (Complex has no bypassing endpoint; the gate precedes mode/stage).
2. **`chat`'s inline no-creds literal unified onto `NO_CREDS_MSG`.** The
   non-streaming `chat` handler used a byte-identical inline string; routing it
   through `chat_auth_gate` swaps it for the `NO_CREDS_MSG` const (same bytes) —
   behavior-preserving cleanup, removes the duplicated literal.
3. **Grounding extracted into `intake_grounding_blocks` (a refactor of
   `interviewer_instructions`).** Architecture cited "reuse the SAME grounding
   blocks (~chat.rs:288–327)"; the cleanest reuse is a shared helper. The emitted
   Fast string is byte-for-byte unchanged (assembly-only refactor), pinned by
   `build_intake_instructions_fast_equals_interviewer_verbatim` and the two
   pre-existing interviewer tests.
4. **Client `mode`/`stage` also added to `sendChat` (non-streaming), not only
   `streamChat`.** Symmetry/completeness; the store uses `streamChat`. Both are
   serde-default server-side so the Fast wire is unchanged.

## Protected invariants (confirmed untouched)

- **`interviewer_instructions` output byte-identical** (Fast = today) — pinned; the
  refactor is grounding-assembly only.
- **`compose_issue_body` + `spec_md_from_issue` round-trip untouched** (D8) — their
  tests are unchanged-green; both modes converge on them.
- **Server stateless** (D1) — no store tables / session state; stage in the request.
- **No forced thinking** (D2) — model/config identical for both modes.
- **No sticky Fast/Complex** (D4) — per-feature at the entry button; nothing stored
  beyond the per-conversation intake.
- **F1 surfaces** (`drive.rs`/`harness.rs`/`task_sink.rs`/`git_fs.rs`/composer UI) and
  **F3 surfaces** (`NewWorkspaceComposerModal`/`useComposerState`) — untouched.

## Handoff (F2)

Ready for **sdd-tester / sdd-reviewer**. The gate (server lib + fmt + clippy + vite
build + new vitest) is green above; the full-suite 139-failure baseline is
pre-existing (proven against `51705bf2`). QA (`qa.sh`, browser, human/staging):
both buttons render + route to distinct behaviors (Fast = one prompt; Complex =
a five-pass interview that reflects the previous answer back); Complex converges
to the same Preview-issues draft as Fast; no-creds surfaces `NO_CREDS_MSG` visibly
on Complex's first turn. This is the basis for the F3 iteration.

---

# F3 — goal-first workspace (Create New Workspace, goal-first). AC 9–11.

- **Feature:** **F3** — the create-workspace entry becomes goal-first, fronting
  the existing composer with a thin goal step. AC 9–11. **Last slice — the spec
  is code-complete after this.**
- **Role:** Developer (sdd-developer)
- **Date:** 2026-07-03
- **Base:** worktree `finish-the-loop` (tip `3b6dbd33`, F1 + F2 already landed)

> **Scope guardrail:** this iteration implements **F3 only**. F1 and F2 were
> already landed (above) and **NONE of their surfaces were touched**: no
> `drive.rs`/`harness.rs`/`task_sink.rs`/`git_fs.rs`, no `chat.rs`/`socratic-intake.ts`/
> `chat-*.ts`/ChatPage buttons, no `useComposerState` internals, no
> `harness-client.ts`. Built to architecture §D (D-C thin goal-step component;
> D3 composer reused via props + reachable via "Skip to details"; D9 goal +
> workdir the only required inputs).

## The seam (architecture §D): a thin `NewWorkspaceGoalStep` fronting the composer

The goal step owns only its local goal/workdir state and hands captured values up
via callbacks; `useComposerState` stays the untouched creation engine, revealed
after "Continue" (seeded) or "Skip to details" (no goal framing). All the pure
decisions live in a DOM-free lib so they are unit-tested without jsdom.

## Pure logic — `crates/agentum-desktop/ui/src/lib/workspace-goal-step.ts` (NEW)

The three gradeable behaviors, all pure (no React/DOM/xterm), so they run without
a jsdom the UI package doesn't ship:
- **`slugifyGoalName(goal, maxWords=6)`** + **`deriveWorkspaceGoalSeed(goal) →
  {goal,name,prompt}`** — the goal → composer-seed mapping (**AC 9** "seed
  name/prompt from the goal"): trims the goal, seeds a short kebab name for the
  workspace-name field, keeps the goal verbatim as the prompt.
- **`deriveGoalIssueDraft(goal) → {title,body}`** — the tracker-step pre-fill
  (**AC 11**): first line → issue title (ellipsis-truncated at 72), whole goal →
  body.
- **`isGoalStepReady({goal,repoId})`** + **`firstGoalStepBlocker(...)`** — the
  required-vs-optional predicate (**AC 10**, D9): goal **and** a workdir target
  (`repoId`) are the only required inputs; the blocker is never silent (names the
  first unmet input, goal before workdir).
- **`OPTIONAL_WORKSPACE_STEPS`** — the three SKIPPABLE steps as data (**AC 10**):
  `worktree`/`scaffold`/`tracker`, each `skippable:true`, each naming the existing
  primitive it reuses (`createWorktree` / `maybeScaffoldSpecFromIssue` /
  `createGithubIssue`). Reuse, don't rebuild.
- **`DEFAULT_COMPOSER_MODAL_PHASE='goal'`**, **`shouldStartAtGoalStep(modalData)`**,
  **`initialComposerPhase(modalData)`**, **`revealDetails(action) →
  {phase:'details',seed}`** — the default-first-screen + "Skip to details" reveal
  decision (**AC 9** / D3), pure/unit-testable state.

**Tests (`workspace-goal-step.test.ts`, NEW, 15):** slug lowercasing/punctuation/
emoji-drop/clamp; the goal→seed mapping (trim + verbatim prompt + empty-slug
edge); the issue-draft title/body + truncation; goal+workdir required and the
first-blocker order (never silent); exactly three skippable steps naming their
primitives; the default goal-first phase + the opinionated-open skip (protects
F1's Tasks hop) + Continue-seeds / Skip-null reveal.

## Component — `NewWorkspaceGoalStep.tsx` (NEW)

A thin default-first screen: a **goal textarea** (focused on open — AC 9's "first
step"; a bare Enter keeps its newline, Cmd/Ctrl+Enter advances), the **workdir
target** via the composer's existing **`RepoCombobox`** (reused, fed the store's
eligible repos), a visible "Next, optionally:" list of the three skippable steps
(so goal-first surfaces the pipeline), and two actions — **Continue** (disabled
until `isGoalStepReady`, names the blocker otherwise) and **Skip to details**
(D3). It imports zero composer internals; it only calls `onContinue(goal,repoId)`
/ `onSkip()`.

## Modal wiring — `NewWorkspaceComposerModal.tsx` (EDITED, F1 wiring kept)

- **`ComposerModalBody`** now holds `phase`/`seed`/`seedRepoId` state (phase lazily
  initialized from `initialComposerPhase(modalData)`), renders
  `NewWorkspaceGoalStep` on the `goal` phase and `QuickTabBody` on `details`, and
  routes Continue/Skip through the pure `deriveWorkspaceGoalSeed` / reveal. The
  `onOpenAutoFocus` handler now prefers `#workspace-goal` (else the repo picker,
  as before).
- **`QuickTabBody`** gains optional `seed`/`seedRepoId` props: `initialName` ⇐
  `seed.name` else `modalData.prefilledName`; `initialPrompt` ⇐ `seed.prompt` else
  `''`; `initialRepoId` ⇐ `seedRepoId ?? modalData.initialRepoId`. **F1's
  `...initialStartGatedRunProp(modalData)` spread is untouched** (goal-path opens
  carry no `startGatedRun`, so it stays `{}` — the user arms the toggle after
  filing an issue, exactly as before). A one-shot effect pre-fills the composer's
  **existing** create-issue form (title+body) from the goal via the public
  `onCreateIssueTitleChange`/`onCreateIssueBodyChange` callbacks (**AC 11**;
  seed-gated so "Skip to details" is byte-identical).

## AC coverage (what's wired-and-unit-tested vs qa.sh/human)

- **AC 9** (goal input is the first step; no repo/branch required before the goal):
  **wired + unit-tested.** Goal textarea is the default first screen and focused
  on open; `initialComposerPhase({})==='goal'`; the seed mapping + reveal decision
  are pinned. Typing the goal is never gated by the repo field.
- **AC 10** (worktree/scaffold/tracker optional & skippable; goal+workdir the only
  required inputs): **wired + unit-tested.** `isGoalStepReady`/`firstGoalStepBlocker`
  + `OPTIONAL_WORKSPACE_STEPS` pin it; "Skip to details" reveals the composer with
  no seed → an existing folder/branch as-is (worktree-creation skipped). None
  blocks creation.
- **AC 11** (all-accepted → can run criteria 1–8 without further setup): **the
  wiring + the pure seed/draft logic are delivered and unit-tested** — the goal
  seeds name + workdir + the create-issue form, and the composer's existing
  create-issue → scaffold → "Start gated run" toggles then reach `start_work`'s
  precondition set (worktree + linked github.com issue + scaffolded spec + backlog)
  with no retyping. **The full run-it-end-to-end (file issue → scaffold → arm gated
  run → green gate) is a qa.sh / human browser check**, not something the
  autonomous gate exercises (it needs the installed app + a real repo).

## Build + test results (observed)

Cargo lives at `~/.cargo/bin`; no Rust was touched, so no cargo run was needed.

| Gate | Command | Result |
|---|---|---|
| UI build | `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix crates/agentum-desktop/ui` | **built in ~1m19s (vite + tsc typecheck green).** |
| F3 vitest (new) | `npx vitest run src/lib/workspace-goal-step.test.ts` | **15 passed / 0 failed.** |
| F3+F1+F2 pure suites | `npx vitest run src/lib/{workspace-goal-step,socratic-intake,start-gated-run-precondition,composer-modal-props,issue-side-effect-gate}.test.ts` | **34 passed / 0 failed (5 files)** — F1 (`start-gated-run-precondition` 4, `composer-modal-props` 2, `issue-side-effect-gate`) + F2 (`socratic-intake`) **stayed green**. |
| UI vitest (full, diligence) | `npx vitest run` | **5761 passed / 139 failed (43 files).** The 139 failures are the **identical pre-existing baseline** (proven on `51705bf2`/`3b6dbd33`); F3 added **+15 passing** (5746 → 5761), **0 new failures**. Only `workspace-goal-step.test.ts` imports my modules and it passes; no existing test renders the modal/goal step (no jsdom), so there is no regression surface. |

## Deviations from architecture §D (with rationale)

1. **Opinionated-open gate (`shouldStartAtGoalStep`/`initialComposerPhase`).** §D
   makes the goal step the "default first screen" but didn't spell out when it is
   NOT the default. I gate it so an opinionated open — F1's Tasks-page gated-run
   hop (`startGatedRun`), a create-from linked item, a prefilled name, or a pinned
   base branch — skips straight to `QuickTabBody` (details). This keeps F1's Tasks
   hop **byte-identical**, honors D3 (the mechanics-first composer stays reachable),
   and keeps goal-first the default only for the *plain* create entry (AC 9). The
   gate is a pure, unit-tested predicate.
2. **"Pre-offer the three optional steps" = a visible list + a seeded create-issue
   form, not auto-run steps.** §D's diagram says Continue should "pre-offer the
   three optional steps." I surface them two ways: (a) a visible "Next, optionally:"
   list in the goal step (from `OPTIONAL_WORKSPACE_STEPS`), and (b) on Continue,
   pre-fill the composer's *existing* create-issue form from the goal so the tracker
   step is one-click. I deliberately did **not** auto-open/auto-run the form — the
   form stays closed and fully skippable (AC 10), the pre-fill is invisible until
   the user opens the existing affordance. This is the low-risk "wiring" for AC 11;
   the run-it-through is the qa.sh/human check.
3. **The goal step's workdir picker reuses `RepoCombobox` fed the store's eligible
   repos, not the composer's full host-scoping.** §D says "reuse the composer's
   existing repo picker." I reuse the exact `RepoCombobox` component with the same
   `Boolean(repo.path)` eligibility filter `useComposerState` uses, rather than
   instantiating `useComposerState` twice (which D-C/D3 forbid touching). Host
   scoping remains fully available in the revealed composer; the goal step only
   captures an initial `repoId`.

## Protected invariants (confirmed untouched)

- **`useComposerState` internals** — reused via props only; **never edited** (D3).
  The composer stays the creation engine and stays reachable ("Skip to details").
- **F1's `initialStartGatedRunProp` wiring** in `QuickTabBody` — intact; the
  goal-first path carries no `startGatedRun`, so it resolves to `{}` as before.
- **F1 surfaces** (`drive.rs`/`harness.rs`/`task_sink.rs`/`git_fs.rs`,
  `useComposerState.ts` internals, `harness-client.ts`) and **F2 surfaces**
  (`chat.rs`/`socratic-intake.ts`/`chat-*.ts`/ChatPage buttons) — **untouched.**
- **Reuse, don't rebuild** — `createWorktree` / `maybeScaffoldSpecFromIssue` /
  `createGithubIssue` and `RepoCombobox` are the existing seams the goal step
  fronts; **no new server surface** (AC 11 is a re-sequencing over existing
  primitives).

## Handoff (F3)

Ready for **sdd-tester / sdd-reviewer**. The autonomous gate (UI build + tsc
typecheck + the new + F1 + F2 vitest suites) is green above; the full-suite
139-failure baseline is pre-existing (proven against `51705bf2`/`3b6dbd33`), and
F3 added +15 passing with 0 new failures. **This completes spec 008's
implementation** (F1 + F2 + F3 all landed). QA (`qa.sh`, browser, human/staging):
goal-first wizard completes with **all optional steps skipped** (workspace opens
on an existing folder) AND with **all accepted** (worktree + issue + spec present);
an all-accepted workspace can immediately run criteria 1–8 — the "Start gated run"
toggle armable with zero further setup (AC 11).
