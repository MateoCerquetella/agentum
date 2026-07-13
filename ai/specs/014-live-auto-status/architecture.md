# Architecture — Spec 014: Live auto-status + agent-attention signal

- **Spec:** `ai/specs/014-live-auto-status/spec.md` (Status: PM → Architect)
- **Author:** Architect (autonomous /sdd-loop, iteration 2), 2026-07-09
- **Grounding:** every `path:line` below was verified against the `origin/develop` snapshot (v0.67.0, `8fb7eb16`). **This worktree's base (v0.57.0) is unusable for the build — implementation MUST branch off fresh `origin/develop`** and re-verify cited lines there (they may drift a few lines; the anchors are function names).

## 0. Shape of the change

Four slices, two crates-worth of change, zero new write mechanics:

```
                     ┌────────────────────────────────────────────────┐
                     │ agentum-server                                 │
  session.started ──►│ tracker_sync reactor ──┐                       │
  gh pr poller ─────►│ tracker_sync poller ───┤                       │
  harness drive ────►│ transition_tracker ────┼─► apply_tracker_      │
  MCP report_status ►│ tool_report_status ────┤   transition()        │──► gh / Linear / board
  planning (Todo) ──►│ board_goals/harness ───┘   [F1: emits          │    (unchanged writes)
                     │                             tracker.phase_     │
  agent.awaiting ───►│ tracker_attention (F4) ──► changed on Applied] │
  session.crashed ──►│  (NEW worker)         ──► apply_blocked_      │
                     │                            transition()        │
                     │                            [F1: emits          │
                     │                             tracker.blocked]   │
                     └───────────────┬────────────────────────────────┘
                                     │ broadcast bus → existing WS /api/events
                     ┌───────────────▼────────────────────────────────┐
                     │ desktop UI                                     │
                     │ F2: tracker-phase chip (persisted trackerPhase │
                     │     from /api/worktrees/detected + live event  │
                     │     overlay)                                   │
                     │ F3: ProjectViewWrapper debounced re-fetch      │
                     └────────────────────────────────────────────────┘
```

The write seam, monotonic guard, binding model, and launch path are untouched. This spec adds **one emission choke point, one background worker, three JSON keys, one chip, one hook**.

---

## 1. Decision Q1 — emission point: **inside the seam** (`apply_tracker_transition` / `apply_blocked_transition`)

### Decision

Emit `tracker.phase_changed` / `tracker.blocked` **inside** `apply_tracker_transition` (`crates/agentum-server/src/task_sink.rs:813`) and `apply_blocked_transition` (`task_sink.rs:907`), via a new required parameter:

```rust
// task_sink.rs — lives next to the seam it serves.
/// Fire-and-forget emission coords for the spec-014 tracker bus events.
pub struct TrackerEmit<'a> {
    pub bus: &'a tokio::sync::broadcast::Sender<agentum_core::Event>,
    /// The bound workspace, when the caller knows it (reactor / poller /
    /// attention worker). None for tracker-coord-only callers (harness,
    /// MCP, planning) — consumers then join on `tracker_url`.
    pub worktree_id: Option<&'a str>,
}

pub async fn apply_tracker_transition(
    store: &Store, provider: &str, tracker_id: &str,
    tracker_url: Option<&str>, phase: TrackerPhase,
    emit: TrackerEmit<'_>,
) -> anyhow::Result<TransitionResult>
```

Mechanically: rename the current body to a private `transition_inner(...)`; the public fn calls it, and on — and **only** on — `Ok(TransitionResult::Applied)` does `let _ = emit.bus.send(event)`. Same wrapper shape for `apply_blocked_transition`. `broadcast::Sender::send` is synchronous and non-blocking; the ignored `Err` (zero receivers) makes fire-and-forget structural, not disciplinary. `Skipped` and `Err` emit nothing (AC 2 — note: a partial write that folds into `Skipped(...)`, e.g. label applied + Projects write failed, also emits nothing by AC-2 fiat; the persisted-phase re-fetch reconciles).

### Tradeoffs weighed

| | Inside the seam (CHOSEN) | At the call sites |
|---|---|---|
| "Transition without emitting" | **Unrepresentable.** The `emit` param is required; the compiler forces every existing *and future* caller to provide a bus. To skip emission you'd have to pass a dummy channel — visible in review. | One `bus.send` block duplicated at **six** sites; a seventh future caller silently forgets and the bus lies by omission. Exactly the failure mode the PM told us to make hard. |
| Only-successful emission (AC 1/2) | Enforced at **one** `matches!(Applied)` arm. | Re-derived six times against `Ok(Applied)` vs `Ok(Skipped)` vs `Err` — six chances to emit on a skip. |
| Churn | Signature change at 6 call sites + ~8 existing tests gain a throwaway `broadcast::channel(8).0`. Mechanical, compiler-guided. | Zero signature change, 6 hand-rolled emit blocks. |
| Layering | `task_sink` gains a dep on `tokio::sync::broadcast` + `agentum_core::Event` — both already crate deps; **no** route-module dependency is added (`task_sink.rs:585` documents that rule; the `worktree_id` is caller-supplied precisely so the seam never reads the registry). | n/a |

### The six call sites (complete enumeration; the compiler will confirm)

| Call site | `worktree_id` passed | Bus source |
|---|---|---|
| `crates/agentum-server/src/tracker_sync.rs:153` (session-start reactor) | `Some(&worktree.id)` | thread `&bus` from `run_session_start_reactor` (`:182`) into `react_to_session_start` |
| `crates/agentum-server/src/tracker_sync.rs:379` (`drive_and_persist`, poller) | `Some(worktree_id)` | **`run_pr_merge_poller` gains a `bus` param** — update the spawn at `crates/agentum-server/src/lib.rs:513–518` to pass `bus.clone()` |
| `crates/agentum-server/src/harness/drive.rs:388` (`transition_tracker`) | `None` | `state.bus` |
| `crates/agentum-server/src/routes/harness.rs:425` (planning → Todo) | `None` | `state.bus` (thread from the route handler if `ensure_spec` only holds `store`) |
| `crates/agentum-server/src/routes/board_goals.rs:605` (planning → Todo) | `None` | `state.bus` |
| `crates/agentum-server/src/routes/mcp.rs:1201` (`agentum_report_status`) | `None` | `state.bus` |

Blocked-path caller: `crates/agentum-server/src/harness/drive.rs:322` (retries exhausted) — passes `state.bus`, `worktree_id: None`, `with_comment: true` (see §3).

**Emission-vs-persist ordering:** the reactor/poller persist `tracker_phase` (`routes/worktrees.rs:179 persist_tracker_progress`) *after* the seam returns, so the event can reach the UI before the registry write lands. Harmless by design: the event is an overlay hint; any worktrees re-fetch reconciles from the persisted value (spec risk "lossy bus").

**Wire form:** add `impl TrackerPhase { pub(crate) fn wire_str(self) -> &'static str }` in `task_sink.rs`; `tracker_sync::tracker_phase_wire` (`tracker_sync.rs:57`) becomes a delegating thin wrapper (keeps its round-trip test intact; avoids a `task_sink → tracker_sync` import).

---

## 2. Decision Q5 — event vocabulary: **distinct `tracker.blocked` kind** (not a `blocked: bool` flag)

### Decision

Two kinds on the existing `agentum_core::Event` shape (`crates/agentum-core/src/lib.rs:428–459` — `kind: String` + optional session fields + `payload: serde_json::Value`; both new kinds fit with zero core-crate changes):

```jsonc
// kind: "tracker.phase_changed" — every successful pipeline write
{ "worktree_id": "repoId::/abs/path" | null,
  "provider": "github" | "linear" | "board",
  "phase": "todo" | "in_progress" | "in_review" | "ready_to_test" | "done",
  "tracker_url": "https://github.com/o/r/issues/N" | null }

// kind: "tracker.blocked" — every applied status/blocked escalation
{ "worktree_id": string | null,
  "provider": "github",
  "tracker_url": string,
  "reason": string }   // the caller's gate_label: "unit gate" | "session crash" | "awaiting input"
```

### Why not one kind with `blocked: bool`

1. **The seam can't truthfully fill `phase` on a blocked write.** `apply_blocked_transition` deliberately knows nothing about the current pipeline phase (`status/blocked` is orthogonal, `task_sink.rs:286–290`). A `tracker.phase_changed{phase: null, blocked: true}` is a phase-changed event in which no phase changed — a vocabulary lie, and it forces every consumer to handle a null phase.
2. **Kind-dispatch is the codebase idiom.** `comment_bridge.rs:44` matches on kind strings; the UI narrows on `ev.kind` (`server-session-activity.ts:48`, `server-worktree-activity-map.ts:151`); `Event::to_watchdog` routes by kind prefix (`agentum-core/src/lib.rs:465`). A boolean discriminator inside one kind fights every existing consumer pattern.
3. **The clear needs no third signal.** PM D2's clear is the idempotent phase re-apply — a *real* write through the pipeline seam, which emits a *real* `tracker.phase_changed`. So AC 11's chip contract is a two-line payload-only reducer: `tracker.blocked` ⇒ attention on; any `tracker.phase_changed` for the same worktree/url ⇒ phase = payload.phase, attention off. No follow-up fetch (PM constraint satisfied).
4. F3 subscribes to both trivially: `kind.startsWith('tracker.')` — a blocked write also moves the Projects card (`github_mark_blocked_with_board`, `task_sink.rs:754`), so the board should re-fetch on it too.

**Non-persistence decision:** `tracker.*` events are bus-only — NOT written to the events table, no connect-time replay (`routes/events.rs:77` replays only `agent.*` snapshots). Cold truth for the chip is the persisted `trackerPhase` (AC 4); events are deltas. `Event::to_watchdog` already returns `None` for non-`watchdog./session.` kinds, so the watchdog feed stays clean — no change needed there.

---

## 3. F4 — the attention worker

### Siting: `crates/agentum-server/src/tracker_attention.rs` (server crate, sibling of `tracker_sync.rs`) — NOT the watchdog crate

`comment_bridge.rs` lives in `agentum-watchdog`, but the F4 worker must call `task_sink::apply_blocked_transition` / `apply_tracker_transition` and the registry helpers (`routes::worktrees::find_tracker_worktree_by_path`, `TrackerWorktree`) — all `agentum-server` items. `agentum-server` already depends on `agentum-watchdog` (it spawns the Watchdog, `lib.rs:471`); a watchdog-crate worker calling server code is a **dependency cycle**. The server crate consumes the watchdog's *events* (kind strings on the shared bus — no compile dependency on the emitter), exactly as `tracker_sync`'s reactor already does with `session.started`. The spec's "sibling of comment_bridge" is satisfied in *shape* (bus-subscriber loop, lag-tolerant, per-key dedupe map — copy `comment_bridge.rs:19–58`), and "next to tracker_sync.rs" in *location*.

Register `pub mod tracker_attention;` in `lib.rs` (beside `pub mod tracker_sync`, `lib.rs:55`) and spawn in `spawn_background_workers` right after the poller block (`lib.rs:510–518`), with `store` + `bus.clone()`.

### Worker design

```rust
// tracker_attention.rs
const ATTENTION_SWEEP: Duration = Duration::from_secs(30);          // timer granularity
const BLOCKED_COMMENT_COOLDOWN: Duration = Duration::from_secs(3600); // PM D2, named constant
fn attention_after() -> Duration   // AGENTUM_ATTENTION_AFTER_SECS, default 600 (PM D1)

pub async fn run_tracker_attention_worker(store: Arc<Store>, bus: broadcast::Sender<Event>)
```

**Loop:** `tokio::select! { ev = rx.recv() => handle(ev), _ = interval(ATTENTION_SWEEP).tick() => sweep() }`. One coarse sweep tick, **no per-session spawned timers** — a 10-minute threshold does not need sub-30s precision, and a single tick keeps the worker O(awaiting-sessions) with zero cancellation machinery. Lagged receiver: log + continue (comment_bridge's D-09).

**State (in-memory; reset-on-restart accepted per handoff):**

```rust
struct Ledger {
    /// session → when it entered awaiting_input. Cleared on
    /// agent.working / agent.input_resolved / agent.finished /
    /// session.started / session.crashed(handled).
    awaiting_since: HashMap<Uuid, Instant>,
    /// worktree_id → episode. Keyed by WORKTREE (the issue is the write
    /// target): two sessions in one workspace share one episode, so a
    /// double-crash can't double-comment.
    episodes: HashMap<String, Episode>,
}
struct Episode { active: bool, last_comment_at: Option<Instant> }
enum Fire { Skip, LabelAndComment, LabelOnly }
```

**Pure decision core** (unit-tested, no IO, no time mocking — pass `now: Instant`):
- `fn due(awaiting_since, now, threshold) -> bool`
- `Ledger::begin_episode(&mut self, worktree_id, now, cooldown) -> Fire` — `Skip` if already `active` (one signal per episode, AC 9); `LabelOnly` if a NEW episode starts within `cooldown` of `last_comment_at` (crash-loop guard, AC 10); else `LabelAndComment` (stamps `last_comment_at`).
- `Ledger::end_episode(&mut self, worktree_id) -> bool` — true ⇒ the caller re-applies the phase (clear only when something was actually flagged; a transient answered prompt fires nothing).

**Event handling (IO shell, mirrors `react_to_session_start`, `tracker_sync.rs:137–176`):**
- `agent.awaiting_input` → `awaiting_since.entry(id).or_insert(now)`. No registry read yet (cheap on chatty streams).
- sweep tick → for each due session: `store.get_session_by_id` → `find_tracker_worktree_by_path(&session.workdir)` (fall back to `session.worktree_path` if the workdir misses) → unbound ⇒ silent no-op (fail-closed) → `begin_episode` → fire.
- `session.crashed` → immediate (no threshold) same resolve → `begin_episode` → fire. `gate_tail` = `payload.signature` (the comment_bridge extraction, `comment_bridge.rs:106–114`); `feature_name` = session name; `gate_label` = `"session crash"` / `"awaiting input"`; `attempts` = 1. (The blocked comment template's "failed the {gate_label} after N attempt(s)" wording reads slightly awkwardly for a crash — accepted v1; do NOT fork the body builder.)
- `agent.working` / `agent.input_resolved` / `session.started` → remove `awaiting_since`; resolve worktree; if `end_episode` ⇒ **clear**: read the worktree's persisted `tracker_phase`, `parse_tracker_phase`, and if `Some(phase)` call `apply_tracker_transition(..., phase, TrackerEmit{bus, Some(&wt.id)})` — the re-apply intentionally bypasses `next_phase_write` (rank-equal is the point) and re-applies the persisted value **verbatim**, so it can never advance or regress; the `gh issue edit` remove-set drops `status/blocked` for free (`task_sink.rs:516–527`) and the resulting `tracker.phase_changed` clears the chip. `tracker_phase == None` ⇒ skip (never fabricate a phase).
- `agent.finished` → clears the awaiting timer only, NOT an episode (AC 10 lists the exact clear conditions; a finished turn after a blocked episode is not "recovered").

**Comment suppression seam change:** `apply_blocked_transition` gains `with_comment: bool`, threaded into `github_mark_blocked_with` (`task_sink.rs:719`) to skip only step 3 (the `gh issue comment`); the label edit and Projects Blocked-column write are unchanged. The harness caller (`drive.rs:322`) passes `true`. This is the *only* behavioral touch inside the blocked write path.

**Never-halt:** every write is awaited inside this worker only (bounded by `run_gh`'s 30 s timeout, `task_sink.rs:616–621`); failures log and drop. The watchdog itself is unaffected — its bus send is already fire-and-forget.

---

## 4. F2 — API exposure + shared type (AC 4)

The UI's worktree list comes from **`GET /api/worktrees/detected`** (`ui/src/runtime/server-worktree-client.ts:8`), whose rows are built by `scan_git_worktrees` (`crates/agentum-server/src/routes/worktrees.rs:839–908`) — and that JSON today carries `linkedIssue`/`linkedPR` but **no tracker keys**. (`GET /api/worktrees` already serializes `trackerProvider/trackerUrl/trackerPhase` because they're typed registry fields, `worktrees.rs:63–68` — nothing to do there.)

**Server change (one spot):** in the `serde_json::json!` row at `worktrees.rs:872–905`, add three keys from the registry `meta`:
```rust
"trackerProvider": meta.and_then(|m| m.tracker_provider.clone()),
"trackerUrl":      meta.and_then(|m| m.tracker_url.clone()),
"trackerPhase":    meta.and_then(|m| m.tracker_phase.clone()),
```
**No new registry field, no serde change to the `Worktree` struct** — the alias-free rule (spec 004) is untouched by construction; the existing `old_shape_registry_round_trips_to_none_not_wiped` test (`worktrees.rs:1257`) stays the guard. Unbound worktree ⇒ all three `null` ⇒ no chip (AC 6, fail-closed).

**Shared TS type:** `crates/agentum-desktop/ui/src/shared/types.ts:226` (`export type Worktree`) gains three **optional** fields (the `linkedGitLabMR` backward-compat precedent at `:243`):
```ts
trackerProvider?: string | null
trackerUrl?: string | null
trackerPhase?: 'todo' | 'in_progress' | 'in_review' | 'ready_to_test' | 'done' | null
```
`shared/*` resolves via the vite alias — verify with `bun run build` + `bunx vitest run`, never bare `tsc`.

---

## 5. UI module placement (pure-model + thin-component convention)

### F2 — chip (new files beside their consumers)

| File | Role |
|---|---|
| **NEW** `ui/src/lib/tracker-phase.ts` | Pure model: `TrackerPhaseWire` union; `trackerEventFromFrame(ev)` → `{kind:'phase', worktreeId, trackerUrl, phase} \| {kind:'blocked', worktreeId, trackerUrl} \| null`; `matchEventToWorktree(evt, rows)` (id first, `trackerUrl` fallback — covers harness events with `worktree_id: null`); `deriveTrackerChip(persistedPhase, live)` → `{phase, label, attention} \| null` (null ⇒ render nothing). Mirrors `lib/server-worktree-activity-map.ts` (IO-free, header comment says so). |
| **NEW** `ui/src/lib/tracker-phase.test.ts` | jsdom-free vitest for all of the above (AC 5). |
| **NEW** `ui/src/store/slices/tracker-phase.ts` | `trackerLiveByWorktreeId: Record<string, {phase?: Wire; attention: boolean}>` + `patchTrackerPhase` (sets attention `false`), `setTrackerAttention`, `clearTrackerLive`. Model on `store/slices/server-worktree-activity.ts` (no-op-on-equal, stable empty selector). Register wherever that slice is registered (store root + `AppState` types — mechanical). |
| **NEW** `ui/src/hooks/useTrackerPhaseSync.ts` | `subscribeServerEvents` (`ui/src/runtime/server-events-bus.ts:129`) → parse → match against `worktreesByRepo` rows → patch slice. Model on `hooks/useServerWorktreeActivity.ts` (no extra socket — the shared bus). |
| **NEW** `ui/src/components/sidebar/TrackerPhaseChip.tsx` | Thin badge, `MetadataStatusBadge` styling (`WorktreeCardMetadataStatusBadges.tsx:9`); props `{worktreeId, persistedPhase}`; reads the slice, derives via the pure model; attention variant = red/alert tone. |
| **TOUCH** `ui/src/components/sidebar/WorktreeCardMeta.tsx` | Render the chip in the issue badge row (`:307–316`, beside `IssueStateBadge`) — distinct from the activity dot and open/closed badges (AC 5). |
| **TOUCH** `ui/src/App.tsx:408` | Mount `useTrackerPhaseSync()` beside `useServerWorktreeActivity()`. |

### F3 — board live refresh

| File | Role |
|---|---|
| **NEW** `ui/src/components/github-project/project-view-live-refresh.ts` | Pure coalescer + `export const PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS = 2_000`. Trailing-edge: first `tracker.*` event opens a window and schedules ONE fire at `+windowMs`; events inside the window mutate nothing. Shape it as a pure reducer over `nowMs` (`coalesce(state, nowMs) -> {state, schedule: boolean}`) so vitest asserts "N events in 2 s ⇒ exactly one fire" without timers. |
| **NEW** `ui/src/components/github-project/use-project-view-live-refresh.ts` | Hook: `subscribeServerEvents`, filter `kind.startsWith('tracker.')`, drive the coalescer with `setTimeout`, invoke the passed refetch callback. Subscription lives only while mounted — unmount unsubscribes, which is the "hidden/inactive views fetch nothing" guarantee (ProjectViewWrapper mounts only when the Projects view is shown). |
| **TOUCH** `ui/src/components/github-project/ProjectViewWrapper.tsx` | One hook call passing a refetch closure built from the same values the auto-fetch effect uses (`:161–192`): `doFetch({owner, ownerType, projectNumber, viewId}, /*force*/ true, queryOverride)`. No other edits; no `setInterval` anywhere. |

---

## 6. Test strategy per slice

Reusable patterns: **fake-`gh` subprocess** = `write_fake_gh` in `task_sink.rs::tests` (`:1794` — writes an executable script; the argv-log variant at `:1804/:1845` appends `"$@"` to a file and asserts exact call lines; program passed **explicitly** to the inner fns, no env mutation, no lock; `tracker_sync.rs::tests:727` has its own copy). Env-mutating tests, if any, take `crate::TEST_ENV_LOCK` (`lib.rs:69`) — but the designs below need none.

**F1 (`tracker-phase-event`)** — in `task_sink.rs::tests`:
- `applied_transition_emits_phase_changed_on_bus`: `broadcast::channel(8)`, subscribe, drive the **board arm** with `fresh_store` (the `board_transition_moves_card_status` fixture, `:1725`) → assert exactly one event, kind + full payload. The emission choke point is upstream of provider dispatch, so the hermetic board arm proves it for all providers; the gh transport is already covered by the existing fake-gh tests.
- `skipped_transition_emits_nothing`: unknown board key (`:1755` fixture) + a failing-gh github arm via the inner fns → `rx.try_recv()` is `Empty` (AC 2).
- Existing tests updated mechanically with a throwaway channel.
- Poller/reactor pure decisions unchanged (`tracker_sync.rs` tests stay green as-is).

**F2 (`phase-chip-live`)**:
- Rust: extend the `detected` row assertions (or add one) — registry row with tracker coords ⇒ the three camelCase keys present; no registry row ⇒ `null`s. The alias-free wipe test (`worktrees.rs:1257`) must stay green untouched.
- Vitest (pure, jsdom-free): `trackerEventFromFrame` (both kinds, malformed frames → null), `matchEventToWorktree` (id hit, url fallback, no match), `deriveTrackerChip` (unbound ⇒ null — AC 6; event overlay wins over stale persisted; blocked ⇒ attention; phase_changed clears attention — AC 11).

**F3 (`board-live-refresh`)** — vitest on the pure coalescer: burst of events inside `PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS` ⇒ one fire; events after the window ⇒ a second fire; non-`tracker.*` kinds ignored (hook-level filter unit).

**F4 (`attention-signal`)** — in `tracker_attention.rs::tests` (+ one seam test in `task_sink.rs`):
- Pure ledger: threshold (`due`), one-fire-per-episode (AC 9), cooldown ⇒ `LabelOnly` (AC 10 crash-loop), `end_episode` gating, transient prompt (cleared before threshold) fires nothing.
- Fake-gh (program-explicit, argv-log): `github_mark_blocked_with(..., with_comment=false)` ⇒ label-ensure + label-edit lines, **zero** `issue comment` line; `with_comment=true` ⇒ exactly one comment line (two-crashes-in-cooldown ⇒ label twice, ONE comment — AC 10).
- Clear: the re-apply path drives `github_transition_with` with the persisted phase — the existing `gh_set_status_label_argv_adds_one_removes_the_four_pipeline_and_blocked` (`:1279`) already pins that any pipeline edit removes `status/blocked`; add one flow assertion (blocked argv → then in-progress argv containing `--remove-label status/blocked`).
- Never-halt: failing fake gh ⇒ handler returns, loop continues (mirror `pr_list_via_gh_failure_is_err_never_halts`, `tracker_sync.rs:758`).
- Keep registry IO **out** of the decision layer (the `session_start_decision` pattern) so no test touches `$HOME`.

**Cannot be tested here (human, `qa.sh`):** the live end-to-end board — real issue picked → chip flips In Progress without refresh → Projects card visibly moves after the debounce → pane kill → `status/blocked` + comment on github.com → restart → label clears. That is the spec's `qa.sh` browser scenario with screenshot evidence; also untestable in `verify.sh`: real gh writes, GitHub read-after-write lag behavior, installed-app rendering.

---

## 7. Build order F1 → F4, with invariant checkpoints

> **Branching:** `git worktree add … -b feat/014-live-auto-status origin/develop`. Do not build on this worktree's base.

1. **F1** — `TrackerEmit` + seam wrapper + `wire_str` + all six call sites + poller bus threading (`lib.rs:513`).
   ✅ Checkpoint: `cargo test -p agentum-server --lib` green; emission tests pass; `grep -rn "apply_tracker_transition(" crates/` shows exactly the six callers (compiler-verified anyway); `TransitionResult` values byte-identical (no behavior change beyond emission); no transition awaits the bus.
2. **F2** — `detected` keys + shared TS type + pure model + slice + hook + chip + `WorktreeCardMeta` render.
   ✅ Checkpoint: `bun run build --prefix crates/agentum-desktop/ui` + `bunx vitest run` green; registry struct diff is **empty** (alias-free invariant holds trivially); unbound worktree renders no chip.
3. **F3** — coalescer + hook + one `ProjectViewWrapper` call.
   ✅ Checkpoint: vitest exactly-one-fetch; `grep setInterval` shows no new interval; unmount unsubscribes (no fetch while the Board page is closed).
4. **F4** (LAST per PM D3; demotable to spec 015 with zero rework — F1–F3 have no dependency on it) — `tracker_attention.rs` + spawn + `with_comment` param (+ `drive.rs:322` passes `true`).
   ✅ Checkpoint: all F4 tests green; monotonic guard untouched (`next_phase_write` diff empty); no new `TrackerPhase` variant; harness blocked path behavior unchanged when `with_comment=true`.

`verify.sh` = the spec's: `cargo test -p agentum-server --lib` && UI build && `bunx vitest run`.

---

## 8. Risks & accepted residuals

- **Lossy bus ⇒ stale chip** — by design the chip is persisted-phase + event overlay; any `detected` re-fetch reconciles. Never make the event stream the only truth.
- **GitHub read-after-write lag (F3)** — the debounced re-fetch may show an unmoved card; accepted v1, NO retry/poll loops (spec lock).
- **In-memory ledger resets on server restart** (handoff-accepted): an in-flight awaiting episode is forgotten; additionally a blocked label set *before* a restart won't auto-clear on recovery (the episode map is empty) — the next real phase transition clears it via the remove-set. Documented residual, no mitigation code.
- **Harness/MCP events carry `worktree_id: null`** — the chip joins on `tracker_url`; in the issue-first flow the workspace and its harness features share the issue URL. Where they don't, the chip simply waits for the persisted-phase re-fetch. No fabricated joins.
- **Label churn** — guarded by threshold (10 min), per-worktree episode dedupe, comment cooldown, and clear-on-recovery; fake-gh tests pin no back-to-back duplicates.
- **Comment-body wording for crashes** reuses `blocked_comment_body` ("failed the {gate_label}…") — slightly awkward, accepted over forking a second template.
- **Do not touch:** `spawn_agent_into_pane`, YOLO translation, pane streaming, `next_phase_write`, the registry `Worktree` serde shape.

---

Handoff: this document + spec.md give the Developer everything; open questions are closed (Q1 = seam emission via `TrackerEmit`; Q5 = distinct `tracker.blocked`). Feature order in `feature_list.json` stays `tracker-phase-event` → `phase-chip-live` → `board-live-refresh` → `attention-signal`.
