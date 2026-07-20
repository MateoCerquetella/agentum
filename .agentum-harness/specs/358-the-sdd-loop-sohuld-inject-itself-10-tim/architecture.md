# Architecture — Spec 358: SDD loop stops on agent check-in over MCP

> Grounded against `origin/develop @ 253173ad` (the spec cited `bee8dc2d`; develop
> has advanced, and every cited seam was re-verified at tip — all three spec
> citations still hold at the same lines: `DEFAULT_MAX_STEPS` at
> `routes/sdd.rs:60`, `drive_sdd_loop` at `routes/sdd.rs:350`,
> `loop_step_prompt` at `sdd.rs:165`). **Implement on a fresh branch off
> `origin/develop`** — this worktree's checkout is v0.57.0-era and has none of
> these files; do not edit in place here.

## Components (what changes — three files, one crate)

All changes live in `crates/agentum-server`. Nothing else in the workspace is
touched.

### 1. `crates/agentum-server/src/routes/sdd.rs` — loop mechanics + check-in seam

Verified current shape: `SddLoops` map + `SddLoopHandle { generation, step,
max_steps, abort }` (lines 43–53), monotonic `LOOP_GENERATION` (line 56),
`DEFAULT_MAX_STEPS: u32 = 10` (line 60), `loop_toggle` off-path
remove+abort+emit (lines 239–252), `emit_loop_stopped` (line 304), `run_loop`
generation-guarded cleanup (lines 315–344), `drive_sdd_loop` (lines 350–392).

Changes:

- **`SddLoopHandle` gains one field**: `summary: Arc<std::sync::Mutex<Option<String>>>`
  — shared with the worker the same way `step: Arc<AtomicU32>` already is
  (created in `loop_toggle`, cloned into `run_loop`). Written by a `done:false`
  check-in, consumed (`take()`) by the worker when it emits the next
  `sdd.loop.step` event. `read_loop_state` / the `GET` response shape stay
  untouched.
- **New `pub(crate) async fn agent_checkin(state, session_id: Uuid,
  generation: Option<u64>, done: bool, summary: Option<String>) -> String`** —
  the seam the MCP tool calls. Logic:
  - Lock `state.sdd_loops`; no live entry for `session_id` → return
    `"no active SDD loop on this session; nothing to stop"` (success — AC 1's
    no-loop clause).
  - `generation` present and `!= handle.generation` → return
    `"check-in is from a stale loop activation; ignored"` (success, stops
    nothing — the staleness constraint).
  - `done == false` → store `summary` into `handle.summary`, return
    `"noted; loop continues (step S of M)"`.
  - `done == true` → remove the entry, `abort.abort()` the worker, then
    `emit_loop_stopped(state, id, &name, "agent_completed", steps)` — the
    session name fetched best-effort like `run_loop` does (lines 334–341).
    Removing the handle + aborting the worker (parked in `wait_for_settle`, an
    await point, so abort is clean) is what guarantees "removed before the next
    injection": there is no worker left to inject.
- **Refactor the toggle-off path** (lines 240–249) and the `done:true` arm to
  share one private remove+abort+emit helper so "stop a loop" exists exactly
  once, parameterized by reason (`"toggled_off"` vs `"agent_completed"`).
- **`drive_sdd_loop` gets the `ai/STATE.md` belt** — inserted after the
  status/tmux check (line 366) and before the step event/injection (line 369),
  so it runs before *every* injection including the first:
  - `routes::util::expand_workdir(&session.workdir)` (exists,
    `routes/util.rs:19`) → join `ai/STATE.md` → `tokio::fs::read_to_string`.
  - Any error (missing file, unreadable, no phase line) → fall through silently
    to today's behavior (a `tracing::debug` at most). Parse via a new **pure**
    `fn state_md_phase(content: &str) -> Option<String>` (in `routes/sdd.rs`,
    beside its consumer): find the first line whose trimmed form — after
    stripping a leading `-`, whitespace, and `**` markers — starts with
    `phase:` (case-insensitive); take the first token of the remainder
    (delimited by whitespace or `<`, so `- **phase:** done <!-- … -->` parses),
    strip `*`, lowercase. Phase `"done"` → return `"state_done"` from the
    drive. This matches the real STATE.md shape in the wild
    (`- **phase:** pm  <!-- idle | spec | … | done -->`) and the value set the
    `sdd-status` playbook documents.
- **Thread session id + generation into the prompt**: `drive_sdd_loop` gains a
  `generation: u64` parameter (passed from `run_loop`, which already receives
  it), and the call at line 378 becomes
  `crate::sdd::loop_step_prompt(step, id, generation, &base)`.
- **Step event carries the last summary**: the `sdd.loop.step` payload
  (line 373) additively gains `"summary": <string>` when
  `handle.summary.take()` was `Some` — i.e. step N+1's event carries the
  `done:false` summary reported during step N. First step never has one.
- **Testability seam (needed by AC 5, not speculative)**: `drive_sdd_loop`
  keeps its public shape and becomes a thin wrapper over
  `drive_sdd_loop_with<F, Fut>(state, id, generation, step_counter, max_steps,
  step_fn)` where `F: FnMut(Session, String) -> Fut, Fut: Future<Output =
  StepOutcome>` and `StepOutcome` maps to the existing arms
  (`Settled`/`Crashed`/`TimedOut`/`InjectFailed`). Production wires the
  existing `inject_prompt` + `wait_for_settle` pair **verbatim** (both in
  `harness/drive.rs:1061` / `:1157`, untouched); tests wire a scripted
  closure. Owned `Session`/`String` args keep the generic lifetime-free. This
  is the minimal seam that lets a unit test prove "no check-in → ends at
  `max_steps`" without tmux (unit tests have no tmux; the live-agent pattern
  for real panes is the `#[ignore]` harness test, out of scope here).

### 2. `crates/agentum-server/src/sdd.rs` — the step prompt

`loop_step_prompt` (line 165; exactly one production caller,
`routes/sdd.rs:378`, plus its own test at line 228 — verified by grep) changes
signature to `loop_step_prompt(step: u32, session_id: Uuid, generation: u64,
base_prompt: &str) -> String`. New body keeps "SDD loop step {step} (automated
— no human is watching this pane)" + the base prompt, and **replaces** the
"reply briefly that the SDD loop is complete and stop" tail with:

- END every step by calling the `agentum_sdd_loop` tool on the agentum MCP
  server with exactly
  `{"session": "<uuid>", "generation": <gen>, "done": true|false, "summary": "<one line>"}`
  — `done: true` when `ai/STATE.md` says the current spec's phase is `done` or
  there is no actionable next step (and then do NOT start new work),
  `done: false` otherwise.
- A degrade clause for MCP-unwired tools (bash/aider get the `full_prompt`
  path, `routes/sdd.rs:166–170`): "if you cannot call MCP tools, state that the
  loop is complete and stop" — for them the STATE.md belt is the real stop
  signal, which is exactly why AC 3 exists.

`sdd.rs` needs `use uuid::Uuid` (crate already depends on uuid everywhere).

### 3. `crates/agentum-server/src/routes/mcp.rs` — the tool surface

Verified current shape: `tool_specs()` JSON catalog (line 273) with
`agentum_report_status` at line 564, `ORCHESTRATION_TOOLS` gate list
(line 249) + catalog filter (lines 613–622), `call_tool` match (lines
665–682), the pure-parse pattern `parse_report_status_args` (line 1169) +
thin `tool_report_status` (line 1227).

Changes:

- **Catalog entry** for `agentum_sdd_loop` placed beside `agentum_report_status`
  (after line 577). Schema: `session` (string, required — the session uuid the
  step prompt embeds), `done` (boolean, required), `summary` (string,
  optional), `generation` (integer, optional — echo the value from the step
  prompt; see Decision D2). **Not** added to `ORCHESTRATION_TOOLS`, so it is
  advertised and callable regardless of the orchestration gate (AC 1),
  mirroring how `agentum_sdd`/`agentum_report_status` already behave.
- **`call_tool` arm**: `"agentum_sdd_loop" => tool_sdd_loop(state, &args).await`
  next to the `agentum_report_status` arm (line 680).
- **Pure `fn parse_sdd_loop_args(&Value) -> anyhow::Result<(Uuid, Option<u64>,
  bool, Option<String>)>`** following `parse_report_status_args`: missing
  `session`/`done` or an unparseable uuid is a *caller bug* → `isError: true`;
  everything downstream (no loop, stale generation) is a success string —
  same contract split report_status uses.
- **`tool_sdd_loop`** is a thin view: parse, then delegate to
  `routes::sdd::agent_checkin(...)` — loop mechanics stay in `routes/sdd.rs`
  beside the map they mutate, honoring the repo rule that MCP tools are "a thin
  view over an existing route/store helper, never a reimplementation".

## APIs

- **New MCP tool** `agentum_sdd_loop` (shape above). No new HTTP routes.
- **Existing routes unchanged**: `GET/POST /api/sessions/{id}/sdd/loop`
  behavior and response shapes are byte-compatible; the toggle-off path is
  refactored to share the stop helper but keeps reason `"toggled_off"`.
- **Events** (additive only):
  - `sdd.loop.stopped` payload stays `{reason, steps}`; `reason` gains two new
    values: `"agent_completed"`, `"state_done"`. All existing reasons
    (`toggled_off`, `playbook_missing`, `session_gone`, `session_not_running`,
    `inject_failed`, `session_ended`, `settle_timeout`, `max_steps`) are
    untouched (AC 4).
  - `sdd.loop.step` payload gains optional `"summary"`.

## Data flow

1. `loop_toggle(active:true)` (unchanged apart from creating the shared
   `summary` slot and passing `generation` down) → `run_loop` →
   `drive_sdd_loop`.
2. Per step: fetch session → status check → **NEW belt: read
   `<workdir>/ai/STATE.md`; phase `done` → return `"state_done"`** → emit
   `sdd.loop.step` (+ last check-in summary, if any) → build prompt via
   `loop_step_prompt(step, id, generation, base)` → `inject_prompt` →
   `wait_for_settle` (both untouched).
3. Agent ends its turn by calling `agentum_sdd_loop` over the already-wired
   agentum MCP (`mcp_provision` wires every local claude/codex launch by
   default — no provisioning change needed):
   - `done:false` → summary parked on the handle; worker wakes on settle and
     loops; next step's event carries the summary.
   - `done:true` (generation matches or absent) → handle removed + worker
     aborted + `sdd.loop.stopped{reason:"agent_completed"}` emitted from the
     tool call path. The worker is gone before any next injection.
   - stale generation / no live loop → success text, nothing stops.
4. Loops that never see a check-in and whose STATE.md never says done behave
   exactly as today: cap at `max_steps`, settle-timeout stops, same reasons.

## Important decisions

- **D1 — dedicated tool over an op on `agentum_report_status`** (pins the
  spec's open question). `report_status` is the *tracker* seam: its schema is
  provider/id/url/phase and its contract is best-effort-never-error toward
  external trackers (`mcp.rs:565`). Loop check-in is *server-internal control*
  with different arguments, different staleness semantics, and a hard
  side-effect (aborting a worker). Overloading one tool would bloat its pure
  parser, contradict its "call freely, always non-fatal" description, and
  couple two unrelated lifecycles. Cost of the dedicated tool: one catalog
  entry + one match arm — the registration pattern is designed for exactly
  this.
- **D2 — staleness token travels in the prompt as `generation`**. The map's
  `SddLoopHandle.generation` already exists to distinguish activations
  (`routes/sdd.rs:45–48`); the only way a *check-in* can prove which
  activation prompted it is to echo a token that the step prompt embedded.
  So `loop_step_prompt` embeds `generation` alongside the session id and the
  tool schema takes it as an optional field. Mismatch → ignored-success.
  **Absent `generation` is honored against the current loop** — tradeoff:
  the realistic failure is an agent dropping an argument (then the stop, the
  whole point of the spec, must still work; the STATE.md belt does not cover
  in-progress specs), while the guarded failure (a *stale* agent that *also*
  dropped the field it was explicitly given, with `done:true`, racing a fresh
  activation) needs two simultaneous mistakes and is no worse than the
  already-unqualified toggle-off route. The constraint "a stale generation's
  check-in must not stop a successor" is satisfied as written: a stale
  prompt's check-in carries the stale generation and is ignored. Note: AC 1
  names `{session, done, summary?}` as the accepted shape; `generation` is a
  strictly additive optional field required by the spec's own staleness
  constraint — without a prompt-carried token that clause is unimplementable.
- **D3 — `done:true` aborts the worker directly (reuse of the toggle-off
  path) instead of a flag the worker polls**. Chose direct remove+abort+emit
  over a "done flag checked between settle and inject" because it reuses the
  proven stop mechanics (`loop_toggle` lines 240–249), makes "no further
  injection" true by construction rather than by loop-ordering, and keeps the
  UI state honest immediately (`read_loop_state` reflects the removal). The
  abort-vs-natural-return race is already solved: `run_loop`'s cleanup only
  acts when its own generation still owns the map entry (lines 323–332), so
  whichever side removes the handle first emits the single stop event.
- **D4 — belt check runs before every injection, including step 1**. An
  already-done STATE.md stops the loop at zero injections with `state_done` —
  strictly better than injecting once to ask. Consequence for `qa.sh`: to
  exercise the *MCP* path (`agent_completed` after step 1, per the spec's
  verification section), the QA fixture's STATE.md must have `phase` ≠ `done`
  with nothing actionable; a fixture with `phase: done` will (correctly)
  observe `state_done` and zero prompts instead. Flagging this so QA stages
  the right fixture rather than reporting a false red.
- **D5 — parser and belt live in `routes/sdd.rs`, not a new module**. The
  parser is ~15 lines with one consumer; a new module or a `Playbook`-level
  abstraction would be speculative. `sdd.rs` (prompt building) changes only
  where the prompt text lives today.
- **D6 — no persistence, no UI, no provisioning changes**. Loops are already
  in-memory-only (`SddLoops` in `AppState`); check-in state (summary) shares
  that lifetime. MCP wiring is already default-on for local launches
  (`mcp_provision.rs`), so the tool is reachable with zero launch changes.

## What stays untouched (boundaries)

- `crates/agentum-server/src/harness/drive.rs` — `inject_prompt` (two-step
  submit), `await_repl_ready`, `wait_for_settle`, `SettleOutcome`: the sacred
  autonomy mechanics. Read, cited, not edited.
- `DEFAULT_MAX_STEPS` (= 10), `SETTLE_GRACE`, `SETTLE_TIMEOUT`
  (`routes/sdd.rs:60–66`): values and usage byte-for-byte.
- `loop_toggle` activation path, `read_loop_state`, `GET`/`POST` response
  shapes; `inject` route; `prompt_for`/`tool_is_mcp_wired`.
- `mcp_provision.rs`, `agentum-executor`, `agentum-store` (the setters the
  tests need — `update_status_and_target`, `sessions.rs:225` — already exist),
  all desktop UI (`agentum-desktop/ui`), the TUI repo, `task_sink.rs`.
- `ORCHESTRATION_TOOLS` and the orchestration gate logic (the new tool simply
  isn't in the list).

## Acceptance criteria → plan → tests

Tests live in the existing `#[cfg(test)]` modules of the files they cover and
run under `cargo test -p agentum-server --lib` (verify.sh, plus
`cargo fmt --check`). The routes/sdd.rs tests reuse the module's existing
`fresh_state()`/`seed_session()` helpers (lines 414–463).

| AC | Plan element | Test (name → what it asserts) |
| -- | ------------ | ----------------------------- |
| 1: tool exists beside report_status, ungated | mcp.rs catalog entry + call_tool arm; `parse_sdd_loop_args`; `agent_checkin` | `sdd_loop_tool_is_advertised_regardless_of_the_orchestration_gate` (mcp.rs — mirrors the existing `agentum_sdd` gate test at line 1426); `parse_sdd_loop_args_requires_session_and_done` (mcp.rs, pure) |
| 1: done:true stops before next inject + emits `agent_completed` | `agent_checkin` remove+abort+emit | `checkin_done_stops_loop_and_emits_agent_completed` (routes/sdd.rs): insert a handle whose worker is `tokio::spawn(std::future::pending())`, subscribe the bus, call `agent_checkin(done:true, matching gen)` → map empty, abort handle finished (no injector left), one `sdd.loop.stopped` with reason `agent_completed` |
| 1: done:false leaves loop + summary on step event | `handle.summary` slot + step-event payload | `checkin_done_false_lands_summary_on_next_step_event` (routes/sdd.rs): `agent_checkin(done:false, summary)` keeps the map entry; drive one scripted step via `drive_sdd_loop_with` → `sdd.loop.step` payload carries the summary |
| 1: no loop / stale generation → success, stops nothing | `agent_checkin` early returns | `checkin_without_active_loop_is_ok_and_stops_nothing` + `checkin_with_stale_generation_is_ignored` (routes/sdd.rs): stale gen → entry still present, worker alive, no stop event |
| 2: prompt embeds session id + check-in instruction | `loop_step_prompt(step, id, generation, base)` | `loop_step_prompt_embeds_session_id_and_checkin_instruction` (sdd.rs — evolves the existing test at line 228): contains the uuid, the generation, `agentum_sdd_loop`, `done`, and no longer contains "reply briefly" |
| 3: STATE.md belt → `state_done`; missing/garbled falls through | belt in `drive_sdd_loop` + pure `state_md_phase` | `state_md_phase_parses_bold_list_line_and_ignores_comments` (pure: `- **phase:** done <!-- … -->` → done; `phase: pm` → not done; garbage → `None`); `drive_stops_state_done_before_first_inject` (routes/sdd.rs): seed session with tempdir workdir containing a done STATE.md, flip to Running via `update_status_and_target`, run `drive_sdd_loop_with` with a counting step fn → returns `"state_done"`, zero steps delivered |
| 4: no-check-in loop still ends at `max_steps`; constants/reasons unchanged | `drive_sdd_loop_with` seam; constants untouched | `drive_without_checkin_ends_at_max_steps` (routes/sdd.rs): scripted step fn always `Settled`, no STATE.md, no check-in → returns `"max_steps"` after exactly `max_steps` deliveries. Settle-timeout/other reasons: code paths untouched (diff-scope) — no new test needed beyond the existing toggle tests still passing |
| 5: the four named tests exist | — | The four rows above (stop-on-done, state_done, prompt content, max_steps) are exactly the spec's list |

## Risks & mitigations

- **Stale check-in kills a successor loop** → generation token in prompt +
  mismatch-ignored (D2). Residual absent-field window accepted (D2 rationale).
- **Double `sdd.loop.stopped` when a check-in races the worker's natural end**
  → whoever removes the map entry first wins; `run_loop`'s generation-guarded
  cleanup (lines 323–332) already makes the loser silent. Covered implicitly
  by `checkin_done_stops_loop…` asserting exactly one event.
- **Remote (SSH) sessions: belt reads the local fs, so a remote workdir's
  STATE.md is invisible** → read error falls through silently to today's
  behavior; accepted because MCP provisioning is local-launch-only anyway, so
  remote loops keep the cap backstop unchanged (matches the spec's
  "falls through silently" contract).
- **MCP-unwired tools (bash/aider) are told to call a tool they don't have** →
  the prompt keeps an explicit no-MCP degrade sentence, and AC 3's STATE.md
  belt is their functional stop signal (that is its stated purpose).
- **Agent declares `done:true` prematurely** → accepted by the spec (the
  check-in *is* the feature); the user can re-toggle, and a wrong stop is
  strictly cheaper than eight junk re-injections.
- **STATE.md format drift breaks the parser** → parser is tolerant (list
  marker, bold, trailing comment, case) and its failure mode is the spec's
  mandated fall-through, never a crash; loop-side belt errors are caught and
  ignored per the constraints.
- **`loop_step_prompt` signature change breaks callers** → grep-verified: one
  production caller + one test, both updated in this slice.
- **qa.sh fixture ambiguity** → flagged in D4: stage `phase ≠ done` to observe
  `agent_completed`; `phase: done` correctly yields `state_done` at zero steps.
