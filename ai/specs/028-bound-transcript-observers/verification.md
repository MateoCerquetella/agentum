# Spec 028 — Tester Verification

- **Spec:** `028-bound-transcript-observers`
- **Date:** 2026-07-23
- **Base under test:** `105ca8ad` (full Spec 028 range `4f3c030c^..105ca8ad`)
- **Role:** Fresh Tester, final autonomous attempt after failure 2/2
- **Verdict:** **PASS — all acceptance criteria have independent executable evidence.**

The final lifecycle-linearization fix closes the stale preloaded-request defect from the previous
Tester gate. Agent-task GET/reset hold a weak-keyed per-session Tokio mutex across the authoritative
durable load and transcript operation. Stop/kill, forced delete, and tool PATCH hold the same
boundary from their authoritative load through durable mutation and transcript retirement. A
request therefore completes before mutation and is caught by retirement, or enters afterward and
loads Stopped, non-Claude, or missing durable state.

No nested lifecycle acquisition was found: the shared `stop_session_core` owns the stop/kill/MCP
boundary, while its HTTP and MCP callers do not acquire it separately. The weak registry retains a
lock while any holder or waiter owns a strong reference and opportunistically prunes dead UUID
keys, avoiding permanent historical-session tombstones.

## Independent gates

| Gate | Result |
|---|---|
| `cargo test -p agentum-server transcript_store::tests --lib -- --nocapture` | **PASS — 11/11** |
| `cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture` | **PASS — 2/2** |
| `cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture` | **PASS — 7/7** |
| server-wired watchdog focused test | **PASS — 1/1** |
| generic watchdog reconcile-hook focused test | **PASS — 1/1** |
| `HARNESS_FEATURE_ID=transcript-observer-lifecycle bash .harness/qa.sh` | **PASS — 21/21 isolated server tests**, including the real `RecommendedWatcher` append/retirement leg |
| `cargo test --workspace --lib --exclude agentum-desktop` | **PASS — 839 passed, 0 failed, 2 ignored** |
| `cargo check -p agentum-server -p agentum-watchdog` | **PASS** |
| `cargo fmt --all -- --check` | **PASS** |
| `.harness/feature_list.json` parse and `bash -n .harness/{qa,verify}.sh` | **PASS** |
| blocking-receiver source guard | **PASS — no `spawn_blocking` or `std::sync::mpsc` in `transcript_store.rs`** |
| `git diff --check` and `git diff --check 1e174a57..105ca8ad` | **PASS** |

The full implementation-range inspection found only pre-existing two-space Markdown hard breaks in
the committed Reviewer send-back report; the implementation, worktree, and final Developer diff
are whitespace-clean.

## Acceptance-criterion map

| AC | Verdict | Independent evidence |
|---|---|---|
| 1 | **PASS** | The production list handler remains database-only. The 500-row fixture, including 250 Claude sessions, returns all rows with zero transcript cache entries, observer creations/drops, or directory creation. |
| 2 | **PASS** | Repeated and 16-way concurrent running-Claude reads create exactly one observer. The route selects `Live` only for durable Running Claude state, and update event kind/payload compatibility remains asserted. |
| 3 | **PASS** | `SnapshotOnly` synchronously consumes newly appended complete lines, preserves partial-line behavior, and drops an existing live observer itself. Stopped route reads leave zero observers. |
| 4 | **PASS** | Non-Claude read/reset creates no entry, directory, observer, or consumer. A live-Claude to non-Claude read forgets the prior cache and observer before returning empty. |
| 5 | **PASS** | Reset-first selects pinned/legacy state, advances the cursor to EOF, and exposes only later complete lines without attaching observation. |
| 6 | **PASS** | Production stop/kill/delete/tool-patch regressions and watchdog reconciliation retire the right resources without starting new observers. Deterministic actual-handler schedules park a GET after loading Running Claude, prove all four lifecycle transitions wait on the keyed boundary, then prove stop/kill/tool patch leave zero observers and forced delete leaves zero observers and zero cache entries. |
| 7 | **PASS** | UUID-pinned isolation/promotion, legacy fallback, complete-line parsing, response schemas, and `agent_tasks.updated` payloads remain covered by focused and workspace tests. |
| 8 | **PASS** | Notify delivery remains capacity-one/coalescing; observer drop aborts the Tokio consumer. Already-received wake regressions cover SnapshotOnly, stop, retain, and forget and prove no post-retirement mutation/event/cache recreation. The production watcher leg emits on append and stays silent after retirement. |

## Adversarial lifecycle evidence

The final handler tests exercise the exact schedule that previously failed, not a hand-modeled
approximation:

1. `get_agent_tasks` acquires the UUID lifecycle guard, loads a durable Running Claude row, and
   parks before choosing/calling `read(Live)`.
2. A concurrent stop, kill, forced delete, or tool PATCH is launched.
3. The lifecycle route is proved unable to reach its early transcript-retirement gate while the
   preloaded request holds the boundary; the durable row is still Running/Claude/present.
4. Releasing the GET lets it attach first. The lifecycle route then acquires the same boundary and
   removes that work while committing its authoritative state transition.
5. Completion proves zero live observers for all four paths; forced delete also proves zero cache
   entries and no durable row. The post-tool-patch reread returns empty and cannot reattach Claude.

The registry regression separately proves same-UUID exclusion and that, after holder/waiter
completion, a subsequent UUID acquisition prunes the dead weak key. Code inspection confirms an
`OwnedMutexGuard` keeps the lock's `Arc` alive for holders, and `lock_owned()` keeps the strong
reference alive while waiting, so opportunistic pruning cannot split a live key into two locks.

## Documented environment gates

- `cargo test --workspace --lib` was rerun and is **environment-blocked** in
  `agentum-desktop` because `../../target/release/libsherpa-onnx-c-api.dylib` is absent. The
  authoritative backend substitution excluding `agentum-desktop` is green at 839 passed and 2
  ignored.
- `npm run build --prefix crates/agentum-desktop/ui` was rerun and is **environment-blocked** with
  `vite: command not found` because UI dependencies are not installed. Spec 028 has no UI/browser
  implementation surface.

No implementation source, dependency, state cursor, handoff, or external state was changed by
Tester. This verification report is the only intended worktree change.
