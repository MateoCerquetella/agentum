# Spec 008 — Architecture Blueprint: Finish the Loop

**Self-check passed.** Load-bearing cites re-verified on the `finish-the-loop`
worktree (tip `0e6812f8`). **A1 — the start-work path already ships from spec
005; F1 is instrumentation + the D6 blocked escalation + a live test, not new
plumbing.**

- **Status:** Architect → ready for Developer. D1–D9 honored.
- **Three architect decisions + one feasibility flag (§1)** — none reopens
  product scope; all change *how/where* work lands.

---

## 0. TL;DR — three slices, one sentence each

1. **F1 (the spine, riskiest):** the start-work wire already exists (005) — F1
   **instruments its two genuinely-silent points** (`await_repl_ready` falling
   through unconfirmed → the prompt fires blind; `wait_for_settle` timing out
   after up to 1800 s → a silent hang), makes the composer's `!repoId`
   early-return loud when a gated run is armed (the #226 edge), adds the **D6
   `status/blocked` label + comment** on the block path (a GitHub-only sibling
   to `apply_tracker_transition`, *not* a 5th `TrackerPhase`), and adds a
   **live start-work-leg test** covering issue → route → session opens → prompt
   lands.
2. **F2:** an explicit `{mode, stage}` on `ChatRequest`; five server-owned
   per-stage system prompts (WHO/WHAT/WHY/done/risks) that wrap the *same*
   grounding blocks; Fast stays `interviewer_instructions` byte-identical
   (pinned); both modes converge on the existing `compose_issue_body`;
   `NO_CREDS_MSG` surfaces by construction (same endpoint).
3. **F3:** a **new thin `NewWorkspaceGoalStep` component fronting the composer**
   (not a step inside the 2.8k-line `useComposerState`); goal + workdir are the
   only required inputs (D9); worktree-creation / scaffold / tracker are three
   skippable steps reusing `createWorktree` / `maybeScaffoldSpecFromIssue` /
   `POST /api/github/issues`.

---

## 1. Corrections, decisions & one feasibility flag (read before building)

- **A1 — the start-work path is already built; F1 is instrumentation, not
  plumbing.** Spec 005 shipped `start_work` (`harness.rs:508`),
  `ensure_spec_and_plan` (`:367`), `start_work_lock` (`harness.rs:62`),
  `find_by_workdir` (`:83`), the two-hop UI (`TaskPage.tsx:4529` →
  `useComposerState.ts:2279` → `harness-client.ts:171`), and spec 007 replaced
  every silent `deriveIssueSideEffectGate` skip with a toast
  (`issue-side-effect-gate.ts:26`, `useComposerState.ts:2291-2295`). **Do not
  rebuild any of it.** The spec's "F1 fixes and hardens" is literal: the
  never-silent map (§B.1) enumerates the *remaining* silent points, which are
  all in the **drive loop** (`drive.rs`) and one **composer guard**.

- **D-A (architect decision) — `status/blocked` is a GitHub-only sibling, NOT a
  5th `TrackerPhase`.** D6 says "status/blocked joins the canonical set … one-
  `status/*`-per-issue now over five labels." The literal-but-wrong reading adds
  `TrackerPhase::Blocked`, which ripples into `board_status_for`
  (`task_sink.rs:253` — the board has no blocked column), the Linear
  `LinearStateMap` (no blocked state by default), and `parse_tracker_phase`
  (`:230` — the F4 MCP tool would start accepting a non-reportable phase).
  **Resolved:** keep `TrackerPhase` at its four pipeline variants (board/linear
  semantics untouched); add a fixed `GITHUB_BLOCKED_LABEL` and a **new**
  `apply_blocked_transition` seam (§B.4). The "one-of-five" invariant is enforced
  purely at the GitHub-label layer by widening the remove-set to all five names.
  Blast radius stays inside `task_sink.rs`'s github arm + one `drive.rs` call.
  This is the *only* extension to the 004 label canon (D6 respected).

- **D-B (architect decision) — F2's stage is an explicit request field, not
  server-derived from turn count.** D1 left this open. Server-deriving stage from
  `messages.len()` is fragile: reflect-back turns and localStorage edits desync
  count from stage, and it couples the stateless server to a client counting
  convention. **Resolved:** `ChatRequest` carries `mode: "fast"|"socratic"` +
  `stage: u8` (§C). The client owns advancement (one stage per user turn, capped
  at 5 — the AC 7 progression is a *client* invariant, unit-tested there); the
  server is a pure `(mode, stage) → system prompt` function (D1's "server
  stateless" preserved). Explicit stage is what makes the per-stage prompt pins
  and the "never skips" test deterministic.

- **D-C (architect decision) — F3 fronts the composer with a new thin
  component.** The handoff asked: new component vs a step inside
  `NewWorkspaceComposerModal`. **Resolved: a new `NewWorkspaceGoalStep` rendered
  as the modal's default first screen.** `useComposerState` (2.8k lines,
  load-bearing: issue linking, host scoping, scaffold/start gating) stays the
  creation engine untouched; a wizard state-machine *inside* it would risk every
  existing caller. "Skip to details" (D3) reveals today's `QuickTabBody`
  (`NewWorkspaceComposerModal.tsx:98`). Mirrors 004-D5 / 005-D5 (front, don't
  replace).

- **F-FLAG — AC 2's never-silent guarantee is architecturally unsatisfiable
  without touching two sacred mechanics; the minimal widening is a readiness-bool
  passthrough.** The *only* code that knows whether the agent's REPL was actually
  ready before the prompt was typed is `await_repl_ready` (`drive.rs:899`), and
  it currently **falls through returning `()`** when it never matches the footer
  — the prompt then fires blind and, if it didn't land, `wait_for_settle` hangs
  up to `settle_timeout_secs` (default **1800 s**, `types.rs:100`) with zero
  events. Making that visible (AC 2) *requires* `await_repl_ready` to report
  ready-vs-timed-out, which `inject_prompt` (`drive.rs:959`) bubbles up — both
  are on D5's sacred list. This is the direct analogue of spec 004's PM finding
  that "zero `drive.rs` changes" was unsatisfiable. **Minimal widening:**
  `await_repl_ready → bool`, `inject_prompt → Result<bool>` (the send logic is
  byte-for-byte unchanged; the bool is pure instrumentation). Per D5 this is
  *permitted* (behavior-preserving instrumentation) but *gated*: it merges only
  with **`harness_live_agent.rs` AND the new start-work-leg live test both green**
  (§B.3). Flagging for the orchestrator: this is the one place F1 must touch a
  sacred mechanic — it stays within D5, but it is the highest-risk change and the
  reason the new live test exists.

- **Confirm to Mateo (D9, one line):** "the 4 can be optional" from the interview
  means **worktree *creation* is optional, not the workdir** — a session is
  `(name, workdir, …)` (`domain_glossary.md`), so a workspace with no workdir is
  not a domain object. Goal + a workdir target are required; the three skippable
  steps are worktree-creation / scaffold / tracker.

- **New risk surfaced — with SDD roles ON by default (006 D1,
  `harness.rs:584-588`), the *first* spawned agent is the PM role gate, not the
  feature agent.** `drive_inner` runs `run_pre_feature_phases` before the feature
  loop (`drive.rs:82-89`). So AC 1's "visible session ≤15 s" and AC 2's "prompt
  lands ≤60 s" apply to the **PM role agent** first (its prompt *is*
  spec-grounded — `build_role_prompt` inlines the spec). The instrumentation must
  therefore be uniform across `spawn_feature_agent`/`spawn_role_agent`/
  `spawn_qa_agent` — which it is, because all three route through the same
  `inject_prompt` (the readiness bool covers every spawn path in one place).

---

## A. Boundaries & build order

**Order: F1 → F2 → F3. F1 ships alone** (F2/F3 may slip; each criterion is
independently gateable per the spec). F2 and F3 have no dependency on each other;
both are value-ordered after F1.

| Feature | May touch | Must NOT touch |
|---|---|---|
| **F1** | `drive.rs` (instrumentation only + the one blocked-transition call), `task_sink.rs` (the new `apply_blocked_transition` + widened remove-set), `harness.rs` (a `record_feature_failure` return tweak if needed), `useComposerState.ts` (the armed `!repoId` toast + surfacing server error detail), a small `harness-client.ts` error-subscription helper, `tests/` (new live test) | `spawn_agent_into_pane` (`provision.rs:107`); the YOLO-marker push; `start_work`'s orchestration sequence (005-shipped, correct); `ChatRequest`; the composer creation engine |
| **F2** | `chat.rs` (`ChatRequest` +2 fields, a `socratic_stage_instructions` sibling, the mode router), `ChatPage.tsx` + `chat-client.ts` (two buttons, stage advancement, persisted stage) | `interviewer_instructions` (`chat.rs:282` — Fast's pinned prompt), `compose_issue_body` (`:992`), `spec_md_from_issue` round-trip, any harness/drive code |
| **F3** | new `NewWorkspaceGoalStep.tsx`, `NewWorkspaceComposerModal.tsx` (render the goal step first + "Skip to details") | `useComposerState.ts` internals (reuse via props only), `POST /api/github/issues`, the scaffold route, any server code |

### The D5 invariant box (F1) — pin verbatim in the PR description

> **Instrumentation is allowed anywhere in `drive.rs`** (HarnessEvent emission,
> error propagation, a bool return for reporting). **The three autonomy mechanics
> change ONLY with `harness_live_agent.rs` AND the new start-work-leg live test
> both green:**
> 1. the **YOLO marker push** (`drive.rs:387-391`, `:475-479`, `:613-617`) —
>    untouched by F1;
> 2. **`await_repl_ready`** (`:899`) — F1 gives it a `bool` return
>    (ready-confirmed); poll/trust-accept/fallback logic byte-identical;
> 3. the **two-step `inject_prompt`** (`:959` — `send_bytes` → `SUBMIT_DELAY` →
>    bare Enter) — F1 changes only its return type to pass the bool through; the
>    send sequence is byte-identical.
>
> **No new spawn path.** Every agent (feature, role, QA) stays on
> `spawn_feature_agent`/`spawn_role_agent`/`spawn_qa_agent` →
> `spawn_agent_into_pane`.

---

## B. F1 seam design — the never-silent run path (the core deliverable)

### B.1 The never-silent map (click → gate)

Every failure point along the path, its **current** behavior, whether it is
**silent**, and the **exact instrumentation** F1 adds. Grounded in the read code.

| # | Failure point (path:line) | Current behavior on failure | Silent? | F1 instrumentation |
|---|---|---|---|---|
| 1 | Tasks hop: `openComposerForItem(item,{startGatedRun:true})` (`TaskPage.tsx:4529`) → modal `initialStartGatedRun` (`NewWorkspaceComposerModal.tsx:123`) | Modal opens, toggle armed | No | Add a vitest: `modalData.startGatedRun → initialStartGatedRun===true`; confirm the Card renders a visible "Start gated run" badge so the armed state is unmistakable (AC 1) |
| 2 | **Composer guard early-return** `if (!repoId || …) return` (`useComposerState.ts:2317-2328`, `:2560`-area) | **Silent `return`** — where the #226 `repoId:''` chat-origin issue (`ChatPage.tsx:472`) lands | **YES** | When `startGatedRun` is armed and the guard trips, `toast.error('Pick a repo before starting a gated run.')` (the specific unmet precondition) instead of a bare `return`. AC 1 closure for the chat-origin edge; the deeper #226 repo-association fix stays deferred with this as the visible re-defer |
| 3 | `createWorktree` throws (`:2437`/`:2650`) | `catch → setCreateError + toast.error` (`:2489-2492`) | No | keep |
| 4 | Armed-but-ineligible gate (`maybeStartGatedRun` `:2291-2295`) | `toast.warning(describeIssueSideEffectSkip('start-gated-run', reason))` | No (007 fixed) | Add the pin test: **every** `IssueSideEffectSkipReason` toasts on the start route (no silent branch) |
| 5 | `start_work` HTTP non-2xx (`harness-client.ts:139-142` throws → catch `:2308-2311`) | `toast.error('Workspace created, but the gated run could not start.')` | No | **Enrich:** surface the server's `ApiError` message (`request()` already appends `— {detail}`) — server messages ("workdir does not exist", "could not plan from the spec") are actionable; the generic string hides them |
| 6 | `alreadyRunning` 200 (`:2303-2307`) | `toast.info(...)` | No (friendly) | keep. **The 2 s ack is the `creating` pending state** (`:2336`/`:2569`, cleared in `finally` `:2494`/`:2711`), NOT this response — `start_work_lock` (`harness.rs:62`) serializes the whole orchestration incl. the network `gh` fetch, so a double-click blocks on the lock |
| 7 | `expand_workdir`/`!is_dir` (`start_work` `harness.rs:517-523`) | `ApiError::BadRequest` → 400 → toast (#5) | No | keep |
| 8 | `fetch_github_issue` fails (`:563-565`) | `ApiError` → HTTP → toast (#5) | No | **Audit result:** the start chain uses **NO Tauri `gh_*` command** — the issue URL/number come from the already-linked item (`deriveIssueSideEffectGate` parses `item.url` purely, `issue-side-effect-gate.ts:36`) and the fetch is the *server's* `gh issue view`. Contrast spec 007's stubbed *detail* fetch — a different surface. Documented so the "gh_* stubs on the Start path" risk is closed with evidence |
| 9 | scaffold/plan IO error (`ensure_spec_and_plan` `:377-412`) | `ApiError::Internal` → HTTP → toast | No | keep |
| 10 | Todo transition fails (`:434-437`) | `warn!` only (pre-registration, no `harness_id` yet) | Semi (log-only) | keep — best-effort by contract (005 C1); the first *visible* label flip is InProgress at spawn (`drive.rs:129`) |
| 11 | `update_backlog_knobs`/register/claim/spawn (`:589-624`) | `ApiError` → HTTP → toast | No | keep |
| 12 | **`drive_inner` early failure** (init.sh `:53-55`; `spawn_*_agent` `:126`/`:686`/`:561`; host missing `:377`) | `bail!`/`anyhow!` → funnels to `drive` (`:33-44`) → `emit_error` + `Failed` state | No (on `/api/harness/events`) **iff a client is watching** | keep the funnel; **add the events bridge (§B.5)** so a composer-started run's `Error`/`Failed` also toasts (the composer navigated to the session view, not the Harness page) |
| 13 | `AgentSpawned` on success (`set_session` `harness.rs:548`) | emits `AgentSpawned` | — | keep (AC 1's "visible session" signal) |
| 14a | **`await_repl_ready` never confirms** (`drive.rs:899-944`) | **falls through, returns `()` — NO event, NO error**; `inject_prompt` then fires the prompt blind | **YES — the deepest silence (AC 2)** | Return `bool`; `inject_prompt` bubbles it; the drive call sites (`:162`, `:694`, `:563`) emit a loud `engine.log(harness_id, Some(feature_id), "⚠ agent REPL never signalled ready in ~56 s — prompt sent anyway; if the pane shows no output the prompt may not have landed")`. F-FLAG (sacred; both live tests gate) |
| 14b | send fails (`inject_prompt` `:980-987`) | `anyhow!` → funnel → `emit_error` | No | keep |
| 15 | **agent produces no output / settle times out** (`wait_for_settle` `:1017-1076`) | returns silently on timeout → gate runs on an unchanged tree | **YES — up to 1800 s silent hang (AC 2)** | `wait_for_settle → SettleOutcome { via: Settled \| Crashed \| TimedOut }` (it is `pub(crate)`, only 3 drive call sites, **not** a sacred mechanic). Drive emits a loud `engine.log` on `TimedOut` |
| 16 | verify/QA gate **red → blocked** (`handle_gate_failure` `:299-308`) | sets `HarnessState::Blocked` + `engine.log`; **no tracker escalation** | **YES on the ISSUE (AC 4)** | Call `apply_blocked_transition` (§B.4): `status/blocked` label + comment (retry count + gate tail). The in-app side is already loud; D6 makes the *issue* loud too |

**The map's spine:** items **#2, #14a, #15, #16** are the four real silences;
everything else is already surfaced and F1 only pins it. #14a + #15 are AC 2; #16
is AC 4; #2 is the AC 1 chat-origin edge.

### B.2 The two-hop UI guarantee (the 2 s ack + visible skips)

- **The 2 s acknowledgment originates in the composer's `creating` state**, not
  the HTTP response. `setCreating(true)` fires at submit entry
  (`useComposerState.ts:2336` / `:2569`), stays true across `createWorktree →
  maybeStartGatedRun`, and clears only in `finally` (`:2494` / `:2711`). Because
  `start_work_lock` serializes (incl. the `gh` fetch), the HTTP response can lag
  on a double-click — so the button's disabled/spinner state (driven by
  `creating`) is the ack. **F1 requirement:** the armed toggle must render a
  distinct pending label ("Starting gated run…") so the ack is unambiguous.
- **`alreadyRunning` renders visibly** as `toast.info` (`:2303-2307`) — a 200,
  not an error loop.
- **Every `deriveIssueSideEffectGate` skip is visible** (`:2291-2295`, spec 007)
  — F1 adds the pin that no reason is silent, and closes the one remaining silent
  gate (#2, the armed `!repoId` guard).

### B.3 The new live test — `tests/harness_start_work_live.rs`

The existing `harness_live_agent.rs` starts from `engine.start(workdir)` +
`harness::drive` (`:97-105`) — it **skips the leg** issue → `POST
/api/harness/start-work` → session opens → prompt lands. The new test covers
exactly that leg.

```rust
//! LIVE: the start-work ROUTE drives a real agent from an issue.
//! #[ignore] — real claude + tmux; run with:
//!   AGENTUM_BROWSER_VERIFY=1 cargo test -p agentum-server \
//!     --test harness_start_work_live -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns a real claude agent; run with --ignored"]
async fn start_work_route_drives_a_real_agent_from_an_issue() { … }
```

- **Boots** the same embedded loopback server (`serve_embedded_loopback_state`)
  as the existing test.
- **Stubs the GitHub fetch, not the agent:** point `AGENTUM_GH_BIN` at a fake
  `gh` (the `task_sink.rs:787` argv-logger pattern) whose `issue view --json
  title,body,url` returns a canned issue with two `- [ ]` boxes, and whose `issue
  edit`/`label create`/`issue comment` calls are logged. Set
  `AGENTUM_GITHUB_CONFIG` to an absent tempdir path (so a dev machine's
  `github.json` can't rename the asserted labels — the exact guard
  `harness.rs:817` already uses). tmux + `claude` stay **real**.
- **Acts:** call the `start_work` handler (or `POST /api/harness/start-work` over
  the loopback addr) with `{ workdir, number: "42" }`; assert the 200 carries
  `runStarted: true` + a `harnessId`.
- **Asserts (the leg):** subscribe to `state.harness.subscribe()` and, within a
  bounded `timeout`, observe (a) `AgentSpawned` (session opens — AC 1); (b) that
  the **injected prompt reached the pane** — `capture-pane` (or the pane log) for
  the spawned session contains a spec/issue marker string from the prompt (AC 2,
  "prompt text visible"); (c) the fake-`gh` log shows the `status/in-progress`
  `issue edit` (AC 3, live label flip from the route). Set
  `harness.sdd.roles.enabled=false` in the test store so the first spawn is the
  **feature** agent (deterministic), and add a companion `#[ignore]` variant with
  roles ON to prove the PM-gate-first path also spawns+prompts (the roles-ON risk
  from §1). Overall ceiling + `cleanup_panes` + `std::process::exit(0)` exactly
  as the existing test (`harness_live_agent.rs:156-191`).

**Cheap unit companions** (in `verify.sh`, always-run): `wait_for_settle` gains a
`TimedOut`-outcome assertion (extend the existing settle tests,
`harness.rs:1316-1366`); the `await_repl_ready` bool is exercised indirectly via
the readiness-log path; the D6 argv tests below.

### B.4 `status/blocked` argv builder (D6) — a GitHub-only sibling

Localized to `task_sink.rs` (D-A). `TrackerPhase` stays four variants.

```rust
// task_sink.rs — a fixed escalation label (NOT configurable: it's not a
// pipeline phase; Linear/board have no blocked column). Red.
const GITHUB_BLOCKED_LABEL: (&str, &str) = ("status/blocked", "b60205");

/// All FIVE canonical status names for the one-per-issue remove-set (spec 008
/// D6): the four CONFIGURED pipeline names + the fixed blocked name. Callers
/// dedupe against the target so a name-collision can't remove the target.
fn all_status_label_names(map: &GithubStateMap) -> Vec<&str>; // 4 pipeline + blocked

/// Blocked → status/blocked, removing the four pipeline names. Mirror of
/// gh_set_status_label_argv (task_sink.rs:451) with target = the fixed label.
fn gh_set_blocked_label_argv<'a>(number: &'a str, slug: &'a str, map: &'a GithubStateMap) -> Vec<&'a str>;

/// gh issue comment argv (pure; body is an argv token, never shell-interpolated).
fn gh_issue_comment_argv<'a>(number: &'a str, slug: &'a str, body: &'a str) -> [&'a str; 7];
// ["issue","comment",number,"--repo",slug,"--body",body]

/// The AC-4 comment: retry count + the gate-output tail, GitHub-collapsible.
fn blocked_comment_body(feature_name: &str, gate_label: &str, attempts: u32, gate_tail: &str) -> String;
// "⛔ **Blocked** — `{feature}` failed the {gate_label} after {attempts} attempt(s).\n\n
//  <details><summary>Gate output (tail)</summary>\n\n```\n{gate_tail}\n```\n</details>\n\n
//  _Posted by the agentum Harness Engine._"

/// GitHub block escalation: ensure status/blocked exists, one edit (add blocked
/// + remove the four pipeline names), then one best-effort comment. Applied iff
/// the LABEL edit succeeds (the comment is secondary — its failure is a logged
/// note, never downgrades Applied). program/map explicit for the fake-gh tests.
async fn github_mark_blocked_with(
    program: &str, slug: &str, number: &str,
    feature_name: &str, gate_label: &str, attempts: u32, gate_tail: &str, map: &GithubStateMap,
) -> TransitionResult;

/// The block-path sibling of apply_tracker_transition (best-effort, never Err).
/// GitHub does the above; board/linear → Skipped("no blocked state") — the ONLY
/// D6 extension (they keep their four-phase columns). Same contract as
/// apply_tracker_transition so drive.rs logs it identically.
pub async fn apply_blocked_transition(
    store: &Store, provider: &str, tracker_id: &str, tracker_url: Option<&str>,
    feature_name: &str, gate_label: &str, attempts: u32, gate_tail: &str,
) -> anyhow::Result<TransitionResult>;
```

**The one-of-five invariant, made structural:** widen `gh_set_status_label_argv`'s
remove loop (`task_sink.rs:468`) from `map.labels()` to
`all_status_label_names(map)` minus target — so *every pipeline transition also
removes `status/blocked`*. This is what clears the blocked label when a reset
feature re-drives to `status/in-progress` (`reset_blocked_features` → spawn →
`transition_tracker(InProgress)`), keeping the board honest in both directions.
`gh` treats removing an absent label as a no-op, so no extra ensure-create is
needed on the happy path (call count unchanged; only the remove-token list grows
by one).

**Call site** (`drive.rs`, `handle_gate_failure` blocked branch `:299-308`): after
`engine.set_state(Blocked)`, add

```rust
let attempts = /* max_retries — block happens exactly at the cap */;
if let Some(provider) = feature.tracker_provider.as_deref() {
    match crate::task_sink::apply_blocked_transition(
        &state.store, provider, &feature.id, feature.tracker_url.as_deref(),
        &feature.name, gate_label, attempts, &tail(output, 2000),
    ).await {
        Ok(r) => engine.log(harness_id, Some(&feature.id), format!("blocked → issue: {r:?}")),
        Err(e) => engine.log(harness_id, Some(&feature.id), format!("blocked issue update failed (non-fatal): {e}")),
    }
}
```

`gate_label` and `output` are already parameters of `handle_gate_failure`
(`drive.rs:290-293`); `attempts` = `config.features.max_retries` threaded from the
two call sites (`:178`, `:229`, both have `config` in scope) — or, cleaner, widen
`record_feature_failure` (`harness.rs:355`) to return `(blocked, attempts)`.
Either is fine; the return-widen is one line and avoids re-reading config.

**D6 test updates (name them — like 004's arity updates):**
`gh_set_status_label_argv_adds_one_removes_exactly_the_other_three`
(`task_sink.rs:967`) now removes **four** (3 pipeline + blocked) — rename to
`…removes_the_three_pipeline_and_blocked` and assert the extra token; arity-update
`gh_set_status_label_argv_uses_configured_names` (`:1143`) and
`…never_removes_the_target_on_name_collision` (`:1181`). New tests:
`gh_set_blocked_label_argv_adds_blocked_removes_four_pipeline`;
`blocked_comment_body_carries_attempts_and_gate_tail`;
`github_mark_blocked_with_fake_gh` (`#[cfg(unix)]`, fake-gh, asserts ensure + edit
+ comment invocations); `apply_blocked_transition_board_and_linear_are_skipped`.

### B.5 The events bridge (surfacing drive-phase errors after a composer start)

`start_work` returns fast (drive is a bg task) and the composer navigates to the
session view — so the drive-phase `HarnessEvent::Error`/`Failed` (#12/#14a/#15)
land on `/api/harness/events` with no one watching. **Minimal bridge:** on a
successful `startGatedWork` the composer subscribes to the harness event stream
filtered by the returned `harnessId` for the run's early lifetime and toasts the
first `Error` (add a small `subscribeHarnessRunErrors(harnessId, onError)` to
`harness-client.ts`, reusing the existing events-WS plumbing the Harness page
uses). This keeps the hard requirement — *every failure point emits a
HarnessEvent* — and adds a lightweight surfacing so the composer-origin flow
honors AC 1's "visible, actionable error" without navigating to the Harness view.

---

## C. F2 seam design — Fast / Complex intake

### The request shape (D-B, D1)

```rust
// chat.rs — ChatRequest gains two fields (both #[serde(default)] → old clients
// and the Fast path are unchanged).
#[serde(default)] mode: Option<IntakeMode>,   // "fast" | "socratic"; None ⇒ Fast
#[serde(default)] stage: Option<u8>,          // 1..=5, socratic only; clamped

#[derive(Deserialize)] #[serde(rename_all = "snake_case")]
enum IntakeMode { Fast, Socratic }
```

Server stays stateless: it maps `(mode, stage)` → a system prompt and nothing
else. The client owns stage advancement (one per user turn, capped at 5) and
persistence.

### The prompt builders

```rust
// chat.rs — Fast is UNCHANGED (the byte-identical pin).
fn build_intake_instructions(
    mode: IntakeMode, stage: u8,
    workdir: Option<&str>, repo_slug: Option<&str>,
    repo_context: Option<&str>, wiki_context: Option<&str>,
) -> String {
    match mode {
        IntakeMode::Fast => interviewer_instructions(workdir, repo_slug, repo_context, wiki_context), // chat.rs:282, verbatim
        IntakeMode::Socratic => socratic_stage_instructions(stage, workdir, repo_slug, repo_context, wiki_context),
    }
}

/// One Socratic pass. Reuses the SAME grounding blocks interviewer_instructions
/// builds (ctx / repo_block / wiki_block / access_rule — chat.rs:288-327) but
/// swaps the "job/Rules" body for a single-topic pass that (a) reflects the
/// previous answer back in one sentence, then (b) asks ONLY this stage's question.
/// Stage 5 ends with the converge instruction (point at "Preview issues").
fn socratic_stage_instructions(stage: u8, …) -> String;
```

The five passes (each covers exactly one topic per AC 7):

| stage | pass | the single instruction |
|---|---|---|
| 1 | **WHO** | reflect nothing yet; establish the persona + the problem they hit |
| 2 | **WHAT** | reflect the WHO back, then pin the desired outcome/behavior |
| 3 | **WHY** | reflect the WHAT back, then the value / why-now / what breaks without it |
| 4 | **done-criteria** | reflect the WHY back, then draw out acceptance criteria (checkbox-shaped) |
| 5 | **risks** | reflect the criteria back, then risks + scope boundaries/non-goals, and **stop asking** — tell the user to click "Preview issues" |

**Convergence (AC 7/8):** after stage 5's user answer, the client stops advancing
and requests the **draft/preview** the same way Fast does — the existing "Preview
issues" → `compose_issue_body` (`chat.rs:992`) path. Both modes therefore end at
identical SDD-shaped issue bodies (`## Problem`/`## Goal`/acceptance-checklist),
from which `spec_md_from_issue` materializes the spec at start-work (D8 — no new
chat-time file write). The server owns none of the stage *state*.

**Fast byte-identical pin (AC 6):** a unit test asserting
`build_intake_instructions(Fast, _, …) == interviewer_instructions(…)` for a fixed
input — the pre-006 body-pin technique. Since the router delegates to the
unchanged function, the pin holds by construction; the test guards against a
future refactor folding Fast into the Socratic path.

**`NO_CREDS_MSG` (AC risk):** both modes hit the **same** `chat`/`chat_stream`
handlers, which gate on `resolve_auth()` up front (`chat.rs:492`, `NO_CREDS_MSG`
at `:76`). F2 must **not** add a separate Complex endpoint — routing Complex
through the same handler makes the no-creds message surface on Complex's first
turn by construction. Pin: `chat_stream_returns_no_creds_when_unauthed` regardless
of `{mode, stage}`.

**D2 (no forced thinking):** the model/config are identical for both modes; the
existing `thinking` opt-in + model picker (`ChatRequest.thinking`, `:121`) apply
to both. Staging — not raw effort — is the quality lift.

### Client (ChatPage)

- Two composer buttons: **Fast feature** (`mode:"fast"`) and **Complex feature**
  (`mode:"socratic"`, `stage:1`). Per-feature choice, no sticky preference (D4).
- Complex advances `stage` by one on each user submit (clamp 5); after stage 5 it
  flips to the Preview-issues draft path.
- **Reload survival (D1, no new store tables):** the interview rides the existing
  localStorage chat history (PR #240); persist `{mode, stage}` on the conversation
  record so a reload resumes at the right pass, and a cleared localStorage cleanly
  restarts (accepted for a solo dogfooder). **Progression is a client invariant**
  (one pass/turn, never skips) → unit-test it in the client (the pure advancement
  reducer), matching AC 7's "unit-tested progression."

---

## D. F3 seam design — goal-first workspace creation

### Structure (D-C, D3)

A new `NewWorkspaceGoalStep.tsx` rendered as the **default first screen** inside
`NewWorkspaceComposerModal` (`:66-96`), fronting `QuickTabBody`. The composer
engine (`useComposerState`) is reused via props, never modified.

```
NewWorkspaceComposerModal
 ├─ (default) NewWorkspaceGoalStep     ← goal textarea + workdir target (repo picker)
 │     ├─ "Continue"  → reveal QuickTabBody, seed name/prompt from the goal,
 │     │                pre-offer the three optional steps
 │     └─ "Skip to details" (D3) → reveal QuickTabBody with no goal framing
 └─ QuickTabBody (existing) → NewWorkspaceComposerCard + useComposerState
```

### Required vs skippable (D9)

- **Required:** goal (free text) + a **workdir target** (the composer's existing
  repo picker — a session needs a workdir).
- **Skippable step (a) — fresh worktree creation:** the composer's `createWorktree`
  on submit; **skip → use an existing folder/branch as-is** (the repo picker
  already supports selecting an existing repo/branch).
- **Skippable step (b) — spec scaffold:** the existing scaffold toggle /
  `maybeScaffoldSpecFromIssue` (`useComposerState.ts:2244`, spec 004 F4).
- **Skippable step (c) — tracker binding:** `POST /api/github/issues` via
  `createGithubIssue` (spec 004 F3) to file an issue *from the goal text*, or link
  an existing one.

### AC 11 — "can run criteria 1–8 with zero further setup"

When goal-first accepts (a)+(b)+(c), the resulting workspace has: a
created/selected **local worktree** + a **linked github.com issue** + a
**scaffolded spec** + a **backlog**. That is exactly `start_work`'s precondition
set — so the composer's "Start gated run" toggle is *armable with no further
setup* (and the goal-first flow may leave it armed to run immediately). No new
server surface: goal-first is a re-sequencing over `createWorktree` +
`createGithubIssue` + the scaffold route. **Reuse, don't rebuild** — the composer
primitives and `useComposerState` are the creation engine.

---

## E. Tradeoffs, risks, invariants

**Carried from the spec:**
- **Silent regression (top risk):** addressed structurally — §B.1 makes silence
  itself a failure at every point; the two real silences (#14a, #15) get loud
  events; the new live test (§B.3) covers the issue→run leg the existing test
  skips.
- **Never cache a failed fetch as success (007):** no new hydration in F1's start
  path (the URL is in hand). F3's goal step files/links issues but reads nothing
  it caches.
- **Tauri `gh_*` stubs on the Start path:** **audited closed** (#8) — the start
  chain issues no Tauri `gh_*` command; the server does the `gh` work. Documented
  as evidence.
- **`repoId:''` #226 edge:** made visible (#2) via the armed-guard toast; deeper
  repo-association fix stays deferred.
- **No-creds path:** surfaces by construction (§C) — Complex rides the same
  auth-gated endpoint.
- **`start_work_lock` serialization:** the 2 s ack is the composer pending state,
  not the HTTP response (§B.2).

**New architectural risks surfaced:**
- **Roles-ON-first-spawn** (§1): the first driven agent is the PM gate; the
  instrumentation is uniform across spawn paths (via `inject_prompt`), and the
  live test covers both roles-on/off.
- **Sacred-mechanic instrumentation (F-FLAG):** the `await_repl_ready`/
  `inject_prompt` bool passthrough is the one place F1 touches a sacred mechanic —
  behavior-preserving, D5-permitted, D5-gated on both live tests. This is the
  single riskiest change; keep the send sequence byte-identical and let the live
  tests be the merge gate.

**Protected invariants confirmed untouched:**
- **One launch path** — no new spawn; every agent stays on `spawn_agent_into_pane`
  (`provision.rs:107`).
- **YOLO translation** — the marker push (`drive.rs:387`) is untouched.
- **Push-based streaming** — F1 reads panes only via the existing
  `capture-pane`/pane-log in the live test's assertions; no polling added to the
  runtime.
- **Best-effort tracker (sacred)** — `apply_blocked_transition` returns only
  `Applied`/`Skipped` (never `Err` for a tracker hiccup); `drive.rs` logs it; a
  blocked issue-update failure never halts the (already-halted) run.

---

## F. Per-feature build/test plan

### F1 — the never-silent run path

**Steps (ordered):**
1. `wait_for_settle → SettleOutcome` (`drive.rs:1017`) + emit a loud `Log` on
   `TimedOut` at the 3 call sites (`:164`, `:322`, `:569`). *Not* sacred — do this
   first (cheapest, biggest silence closed).
2. `apply_blocked_transition` + `gh_set_blocked_label_argv` + comment builder in
   `task_sink.rs`; widen the pipeline remove-set to all-five-minus-target; wire
   the call into `handle_gate_failure`'s blocked branch (`drive.rs:299`).
3. `await_repl_ready → bool` + `inject_prompt → Result<bool>` + the readiness
   `Log` at the drive call sites (**sacred — gated on both live tests**).
   `board_goals.rs`'s `inject_prompt` caller ignores the bool.
4. UI: armed `!repoId` toast (`useComposerState.ts:2317`); surface server error
   detail in the start-failure toast (#5); the `harness-client.ts` events bridge
   (§B.5).
5. The new live test (`tests/harness_start_work_live.rs`).

**`verify.sh` unit assertions (must be green):**
- `wait_for_settle` `TimedOut` outcome (extend `harness.rs:1316-1366`).
- D6 argv: `gh_set_blocked_label_argv_adds_blocked_removes_four_pipeline`; updated
  `gh_set_status_label_argv_…removes_the_three_pipeline_and_blocked`;
  `blocked_comment_body_carries_attempts_and_gate_tail`;
  `github_mark_blocked_with_fake_gh` (`#[cfg(unix)]`);
  `apply_blocked_transition_board_and_linear_are_skipped`.
- UI vitest: `modalData.startGatedRun → initialStartGatedRun`; every
  `IssueSideEffectSkipReason` toasts on the start route; the armed `!repoId` guard
  toasts (colocate under `lib/`; avoid xterm imports — known vitest loader noise).
- `cargo fmt --check` + clippy; `npm run build --prefix crates/agentum-desktop/ui`
  + vitest.

**`qa.sh` browser assertions** (require the browser-QA knob armed —
`AGENTUM_BROWSER_VERIFY` / `browserQaAgentEnabled`, **default OFF** per 005 F3,
else vacuous): Start-gated-run click (both hops) → session visible **or** an
actionable error visible (never nothing); a driven issue shows `status/in-progress`
live; a deliberately-red gate shows `status/blocked` + a comment (retry count +
gate tail) on the issue (D6); the armed toggle renders a visible pending ack ≤2 s.

### F2 — Fast / Complex intake

**Steps:** `ChatRequest` +2 fields → `build_intake_instructions` router →
`socratic_stage_instructions` (5 passes, reuse the grounding blocks) → client two
buttons + stage advancement + persisted `{mode, stage}`.

**`verify.sh`:** `build_intake_instructions(Fast,…)==interviewer_instructions(…)`
byte-identical pin; `socratic_stage_instructions` per-stage prompt pins (each names
exactly its pass + the reflect-back instruction; stage 5 names "Preview issues");
`chat_stream_returns_no_creds_when_unauthed` regardless of mode; client progression
reducer (one pass/turn, never skips, clamp 5). `compose_issue_body` tests
unchanged-green (the convergence endpoint is untouched).

**`qa.sh`:** both buttons render and route to distinct behaviors (Fast = one
prompt; Complex = a five-pass interview that reflects the previous answer back);
Complex converges to the same Preview-issues draft as Fast; no-creds surfaces
`NO_CREDS_MSG` visibly on Complex's first turn.

### F3 — goal-first workspace

**Steps:** `NewWorkspaceGoalStep.tsx` (goal + workdir) fronting the modal;
"Continue" seeds the composer + pre-offers the three optional steps; "Skip to
details" reveals `QuickTabBody`; wire the optional steps to `createWorktree` /
`maybeScaffoldSpecFromIssue` / `createGithubIssue`.

**`verify.sh`:** vitest for the goal-step render + "Skip to details" reveal + the
goal→name/prompt seed mapping; `npm run build` + `tsc` green. No server tests (no
server change).

**`qa.sh`:** goal-first wizard completes with **all optional steps skipped**
(workspace opens on an existing folder) AND with **all accepted** (worktree + issue
+ spec present); an all-accepted workspace can immediately run criteria 1–8 (the
"Start gated run" toggle is armable with zero further setup — AC 11).

---

## Handoff to Developer (sdd-developer)

- **Completed:** all seams line-verified on `0e6812f8`; D1–D9 honored; three
  architect decisions (D-A no-`TrackerPhase::Blocked` sibling; D-B explicit
  `{mode,stage}`; D-C thin goal-step component) + the F-FLAG feasibility note
  (AC 2's never-silent needs the readiness-bool passthrough through two sacred
  mechanics — D5-permitted, D5-gated). The critical framing correction (A1): **the
  start-work path already ships from spec 005; F1 is instrumentation + the D6
  blocked escalation + a live test, not new plumbing.**
- **Do-not-re-litigate:** `TrackerPhase` stays four variants; `status/blocked` is
  a fixed GitHub-only label with an `apply_blocked_transition` sibling and a
  five-name remove-set; the composer 2 s ack is the `creating` pending state; F2
  routes Complex through the *same* auth-gated chat endpoint; F3 never edits
  `useComposerState` internals.
- **First thing to write:** `wait_for_settle → SettleOutcome` + its `TimedOut`
  loud-log (cheapest, closes the 1800 s silent hang; **not** a sacred mechanic, so
  it needs no live test) — then the D6 argv tests, then the sacred
  `await_repl_ready`/`inject_prompt` bool passthrough LAST, behind both green live
  tests.
- **Reviewer focus:** the send sequence in `inject_prompt` is byte-identical (only
  the return type changed); `apply_blocked_transition` is `Ok(Skipped)`-never-`Err`;
  the pipeline remove-set now includes `status/blocked` (the board can't lie in
  either direction); no new spawn path; no `is_public` additions; the armed
  `!repoId` guard no longer returns silently.
- **The single riskiest thing to build first:** the **`await_repl_ready`/
  `inject_prompt` readiness-bool instrumentation** (F-FLAG) — it is the only F1
  change that touches a sacred autonomy mechanic, it is what closes AC 2's deepest
  silence, and it is the reason the new start-work-leg live test exists. Build it
  *after* the two live tests are wired and green, keep the trust-accept/poll/send
  logic byte-for-byte unchanged, and let both live tests be the merge gate.

**Key files:** `crates/agentum-server/src/harness/drive.rs`,
`crates/agentum-server/src/harness.rs`, `crates/agentum-server/src/harness/types.rs`,
`crates/agentum-server/src/task_sink.rs`, `crates/agentum-server/src/routes/chat.rs`,
`crates/agentum-server/tests/harness_start_work_live.rs` (new),
`crates/agentum-desktop/ui/src/hooks/useComposerState.ts`,
`crates/agentum-desktop/ui/src/runtime/harness-client.ts`,
`crates/agentum-desktop/ui/src/lib/issue-side-effect-gate.ts`,
`crates/agentum-desktop/ui/src/components/harness/ChatPage.tsx` +
`runtime/chat-client.ts`,
`crates/agentum-desktop/ui/src/components/NewWorkspaceGoalStep.tsx` (new) +
`NewWorkspaceComposerModal.tsx`.
