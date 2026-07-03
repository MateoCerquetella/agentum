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
