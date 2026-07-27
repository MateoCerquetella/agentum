# Spec 437 — Architecture

- **Spec:** `437-topbar-is-dissapearing-on-harness-gates`
- **Phase:** Architect
- **Date:** 2026-07-23
- **Verdict:** ready for developer

Every existing seam cited below was read in this worktree. The issue's original
`components/harness/HarnessEngine.tsx` path does not exist here; the PM-refined
spec correctly identifies the current implementation under `components/gated-run`
and `hooks`.

## Components

### 1. Worktree-owned harness snapshot continuity

Modify
`crates/agentum-desktop/ui/src/hooks/useWorktreeHarnessRun.ts::useWorktreeHarnessRun`.
Keep its public `{ run, refresh }` return contract, its mount-time
`listHarnesses` read, and its single `subscribeHarnessEvents` subscription. Add
only the state needed to retain confirmed `HarnessStatus` snapshots by the
normalized requested workdir and to order overlapping asynchronous refreshes.

The hook must make these decisions synchronously from its retained snapshots:

- A requested workdir may expose only a run whose normalized `run.workdir`
  matches it. A snapshot from the previously selected worktree must never leak
  into the newly selected worktree.
- Returning to a previously visited owning worktree reuses its last confirmed
  snapshot while the authoritative refresh is in flight, so the existing bar
  does not render a transient `null` frame.
- An event refresh for the active matched harness keeps the last confirmed run
  visible until a newer successful status/list response replaces it. A slow
  older response cannot overwrite a newer response.
- An authoritative list response with no match clears the requested workdir's
  retained snapshot. This preserves the existing behavior for a deleted run or
  a worktree that never owned one.

Reuse
`crates/agentum-desktop/ui/src/lib/harness-run.ts::findHarnessRunForWorkdir`
and its existing `normalizeWorkdir` behavior. Reuse
`crates/agentum-desktop/ui/src/runtime/harness-client.ts::{listHarnesses,
getHarnessStatus,subscribeHarnessEvents}` and the existing `HarnessStatus`,
`HarnessEvent`, and `HarnessEventStream` wire types. Do not introduce a Zustand
slice, a second status model, polling, or another WebSocket.

For focused coverage, create
`crates/agentum-desktop/ui/src/hooks/useWorktreeHarnessRun.test.ts`. Factor the
small snapshot-selection/ordering transition used by the hook into IO-free,
colocated logic so Vitest can drive gate-event and worktree-switch sequences in
the repository's default Node test environment. Named cases:

- `keeps the matched snapshot selected while gate refreshes resolve in order`
- `selects the owning snapshot when switching away and back and clears an unmatched worktree`
- `rejects a stale response and a snapshot owned by another normalized workdir`

Boundary: do not change harness routes, event payloads, gate semantics, session
attachment, run retry/unlink behavior, or the worktree store. In particular,
`crates/agentum-desktop/ui/src/store/slices/worktrees.ts::setActiveWorktree`
continues to restore the active workspace data in one Zustand update; harness
status remains derived by workdir in the hook.

### 2. Existing progress-strip render contract

Keep the production mount and view components unchanged:

- `crates/agentum-desktop/ui/src/components/Terminal.tsx::Terminal` already
  renders one `GatedRunBar` as a root `shrink-0` strip above all workspace
  surfaces whenever the terminal view has an active worktree.
- `crates/agentum-desktop/ui/src/components/gated-run/GatedRunBar.tsx::GatedRunBar`
  already resolves `worktreeId` to `worktree.path`, consumes
  `useWorktreeHarnessRun`, and returns `null` only when there is neither a run
  nor the existing pending-start state.
- `GatedRunBarView` already owns the single
  `aria-label="Gated run progress"` section, calls
  `gatedRunHeadline(run)`, and renders the existing feature-state labels for
  `verifying`, `ready_to_test`, `done`, and `blocked`.

Extend
`crates/agentum-desktop/ui/src/components/gated-run/GatedRunBar.test.tsx` with a
table-driven regression named
`renders exactly one progress region across verifying, ready_to_test, done, and blocked`.
For each status, render the existing view/host with
`renderToStaticMarkup`, assert exactly one progress-region label, and assert the
state's expected headline or feature label. Keep the current no-run assertion
to pin that an unmatched worktree renders no bar.

`crates/agentum-desktop/ui/src/App.tsx::App` and
`crates/agentum-desktop/ui/src/components/error-boundaries/RecoverableRenderErrorBoundary.tsx::RecoverableRenderErrorBoundary`
were also read. `App` keeps the terminal workbench mounted under the same
`Suspense` and error-boundary instances; changing the boundary's `resetKey`
clears boundary error state but does not key/remount its children. No shell,
sidebar, titlebar, worktree-pane, or error-boundary edit is needed.

Boundary: the bar remains exclusive to the active terminal workspace. Do not
show it on full-page views, duplicate it inside a pane, redesign its markup or
styles, or key/remount `Terminal`/`GatedRunBar` per status or worktree.

## APIs

No server, HTTP, WebSocket, store, or component-prop API changes are required.

- Preserve `useWorktreeHarnessRun(workdir: string | undefined):
  WorktreeHarnessRun` and `WorktreeHarnessRun = { run: HarnessStatus |
  undefined; refresh: () => void }`.
- Preserve `GET /api/harness`, `GET /api/harness/{id}`, and
  `WS /api/harness/events` usage through the existing client functions.
- Preserve `GatedRunBar({ worktreeId })` and `GatedRunBarView` props.
- Reuse `HarnessStatus` as the only run snapshot and
  `findHarnessRunForWorkdir` as the ownership rule.

Any IO-free transition exported solely for the new focused test is an
implementation detail of `useWorktreeHarnessRun.ts`; it must not become a new
application service or public store contract.

## Data Flow

1. `setActiveWorktree` updates `activeWorktreeId` and the worktree's restored
   session/tab data; the long-lived `Terminal` receives the new id.
2. `GatedRunBar` resolves that id against `worktreesByRepo` and calls
   `useWorktreeHarnessRun(worktree.path)`.
3. The hook normalizes the requested workdir, synchronously selects only that
   workdir's retained confirmed snapshot, and starts/restarts the existing
   authoritative list plus event subscription effect.
4. `listHarnesses` establishes ownership through
   `findHarnessRunForWorkdir`. Events for the matched `harness_id` trigger the
   existing single-status read; lagged or not-yet-matched events trigger the
   existing list reconciliation.
5. Ordered successful responses replace the requested workdir's retained
   snapshot. While an event response is pending, the prior matching snapshot
   remains selected, so React updates the contents of the same
   `GatedRunBarView` section instead of removing and recreating it.
6. Switching to an unmatched worktree selects no snapshot and therefore no
   bar. Switching back selects the owning worktree's retained snapshot
   immediately, then reconciles it with the latest server status.

## Important Decisions

### D1 — Fix the data hook, not the shell mount

Choose workdir-keyed snapshot continuity in `useWorktreeHarnessRun` over moving
or duplicating `GatedRunBar`. `Terminal` already provides the correct single,
root-level flex strip and `App` does not key-remount the workbench; the unstable
boundary is the hook's one unkeyed React state value while its `workdir` input
changes and asynchronous reads overlap.

### D2 — Retain the last confirmed matching snapshot during refresh

Choose stale-while-revalidate for the already confirmed owning run over
clearing it before each list/status request. Harness events are hints to fetch
the new authoritative snapshot, not evidence that ownership disappeared.
Keeping the matching snapshot prevents the visible collapse; an authoritative
list with no match remains the explicit eviction signal.

### D3 — Use a small instance-local cache, not global application state

Choose a cache scoped to the mounted hook plus a monotonic response order over
a new Zustand harness slice or process-wide subscription manager. This is
enough to survive active-worktree switches in the existing long-lived
`Terminal`, avoids broad store subscriptions and shell renders, and disappears
on unmount. A global cache/provider would be a speculative abstraction for
this regression.

### D4 — Test transitions as pure state plus existing SSR markup

Choose a colocated IO-free transition test and the existing
`renderToStaticMarkup` component pattern over adding jsdom/testing-library.
The UI package currently has neither DOM test dependency. This combination
directly pins response ordering, workdir ownership, return-switch continuity,
exactly-one markup, and all named feature states without adding dependencies.

## Acceptance Criteria Mapping

| ID | Acceptance criterion | Named plan part | Named test / verification |
| --- | --- | --- | --- |
| AC1 | The active terminal workspace has exactly one labelled progress bar for its registered run, including `verifying`, `ready_to_test`, `done`, and `blocked` | Components 1–2 | `GatedRunBar.test.tsx` — `renders exactly one progress region across verifying, ready_to_test, done, and blocked`; existing `renders nothing when no run owns the worktree` |
| AC2 | A matched harness event updates the headline without an absent intermediate frame | Component 1, Data Flow steps 4–5 | `useWorktreeHarnessRun.test.ts` — `keeps the matched snapshot selected while gate refreshes resolve in order`; the table-driven bar test asserts the resulting copy |
| AC3 | Switching away and back restores the owning worktree's latest status while an unmatched worktree has no bar | Component 1, Data Flow steps 1–3 and 6 | `useWorktreeHarnessRun.test.ts` — `selects the owning snapshot when switching away and back and clears an unmatched worktree` and `rejects a stale response and a snapshot owned by another normalized workdir`; existing no-run bar test |
| AC4 | Focused Vitest regression checks exit 0 | Components 1–2 | `(cd crates/agentum-desktop/ui && npm exec vitest run src/hooks/useWorktreeHarnessRun.test.ts src/components/gated-run/GatedRunBar.test.tsx)` |
| AC5 | Desktop UI production build exits 0 | Components 1–2 and unchanged API boundary | `npm run build --prefix crates/agentum-desktop/ui` |

## Risks

- **An older event response overwrites a newer gate state.** Mitigation: assign
  a monotonic order to asynchronous list/status requests and apply a response
  only if it is still current for the active effect/workdir.
- **React retains the prior worktree's run when `workdir` changes.** Mitigation:
  select snapshots by the same normalized workdir ownership rule used by
  `findHarnessRunForWorkdir`; the new test explicitly presents a foreign run
  and expects no selection.
- **Snapshot retention hides actual run removal.** Mitigation: an authoritative
  list response with no matching run evicts that workdir; the existing 404
  fallback still re-lists, and `refresh()` still forces the same reconciliation.
- **The fix increases backend/UI churn.** Mitigation: retain one event stream,
  the existing mount/list and matched-status read rules, and an instance-local
  cache; add no polling, new store subscription, duplicated bar, or shell key.
- **Node-only tests miss browser layout behavior.** Mitigation: the production
  layout is intentionally untouched and already pins `shrink-0`/`z-30` in the
  existing markup test. The new tests cover the changed state logic and exact
  region count; the production build is a final gate. Residual live-webview
  paint timing is accepted because no layout or mount boundary changes.
