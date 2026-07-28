# Spec 023 — Architecture

- **Spec:** 023-gated-run-surfacing-and-issue-unlink
- **Phase:** Architect → Developer
- **Author:** Mateo Cerquetella (Orchestrator, autonomous SDD loop)
- **Date:** 2026-07-17
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/387

> ⛔ **IMPLEMENTATION CONSTRAINT (read first).** All `path:line` refs below are
> against **`origin/develop` (v0.86.0)**. The worktree this was authored in
> (`fix-start-gated-run-on-new-worrkspace-and-unlick`) is **v0.57.0-based, ~274
> commits behind develop** — the files this design edits (`open-created-workspace.ts`,
> `gated-run-ownership.ts`, current `harness/drive.rs`/`types.rs`) **do not exist
> in that checkout**. The Developer phase MUST run on a **fresh worktree cut from
> `origin/develop`** (created OUTSIDE `.claude/worktrees/`, per the worktree-reap
> lesson). Do NOT `git merge`/`rebase` develop into the stale worktree — that is
> the documented STALE-BASE trap that reverts server refactors. This architecture
> doc and the spec are clean new-file adds and land safely regardless.

## System boundaries

Two independent slices, two surfaces, no shared code path:

- **Part A (surfacing)** is **UI-only** in the common case: the server already
  spawns and drives the gated run correctly. The change lives in
  `crates/agentum-desktop/ui` (the workspace view + a store slice + a pure
  decision). One *optional* server touch (see Q1) is explicitly rejected below.
- **Part B (unlink)** is **server-primary**: a clear-tracker primitive +
  persistence + one route in `crates/agentum-server/src/routes/harness.rs`, then a
  thin client method + a UI affordance.

The two slices share nothing but the `HarnessStatus` wire type, so they are wired
as independent harness features and can gate/ship in either order.

## Resolved open questions (architect decisions)

### Q1 — Part A: session↔worktree association vs. by-`harness_id` UI surface → **by-`harness_id`/`workdir` UI surface**

**Decision:** Do **not** set `worktree_path`/`worktree_branch` on the harness
`NewSession` (`drive.rs:442`). Instead, surface the run in the workspace view by
matching the live `HarnessStatus.workdir` (`harness-client.ts` `HarnessStatus`) to
the worktree's path.

**Why:** setting `worktree_path` on the engine session would make it a
first-class worktree session — subject to sidebar listing, user kill/close, and
worktree teardown/orphan-reap. The harness drive loop owns that session's
lifecycle; letting the worktree UI treat it as user-owned risks it being killed or
reaped mid-drive (regressing the `spawn_feature_agent` → `await_repl_ready` →
`inject_prompt` → `wait_for_settle` chain). The by-`harness_id` surface changes
**zero** server/session semantics and reuses infrastructure that already exists:

- `HarnessStatus` already carries `workdir`, `state`, `current_feature`,
  `current_session`, `phase` (`harness-client.ts`, the `HarnessStatus` type).
- `workspace-harness-offer.ts` already establishes the per-worktree harness
  store-slice pattern (resolve worktree by id + connectionId, write a per-worktree
  slice the banner renders) — Part A adds a sibling "gated run pending" slice.
- `WS /api/harness/events` already emits `agent_spawned` (carries `session_id`),
  `state_changed`, `feature_state_changed`, `log` — the surface transitions on
  these (push-based; no poll, honoring the streaming invariant / AC-4).

### Q2 — Part B route shape: `PATCH /api/harness/{id}` vs dedicated route → **`POST /api/harness/{id}/unlink-issue`**

**Decision:** add `.route("/api/harness/{id}/unlink-issue", post(unlink_issue))`
alongside the existing `/{id}/run|init|verify|confirm|files` verbs (`harness.rs:51-55`).
Narrower intent than a general `PATCH {issue_url:null}`, matches the existing
`/{id}/<verb>` convention, and sidesteps designing a partial-update body now. (A
general `PATCH` can come later if more run-field edits are wanted — YAGNI today.)

### Q3 — unlink persistence → **persist to `feature_list.json` (survives restart)**

**Decision:** unlink clears `tracker_provider`/`tracker_url` on every feature in
the in-memory run **and** rewrites `feature_list.json`. The link is stored on disk
(the setter at `types.rs:970-975` writes it), so a mistaken link must be cleared on
disk to stay cleared across a reload/restart — a session-only mute would silently
re-link on restart. This is the desired "I unlinked the wrong issue" semantics.

## Part A — surfacing design

**Data flow (all client-side):**

1. `openCreatedWorkspace` (`lib/open-created-workspace.ts`) already fires
   `maybeOfferWorkspaceHarnessRun({ worktreeId, gatedRun })` fire-and-forget. When
   `gatedRun` is armed **and** the engine took ownership
   (`gatedRunResultOwnsWorktree` → true, decided at the composer call site in
   `useComposerState.ts`), stash a **per-worktree "gated run starting" slice**
   (mirroring the offer slice; keyed by `worktreeId = ${repoId}::${path}`).
2. The workspace view — the component that renders `WorkspaceAgentLauncher` (the
   "Start a session" picker) when a worktree has no active session — consults a
   **pure decision**:

   ```
   deriveGatedRunSurface({
     pendingGatedRun: boolean,           // the slice from step 1
     harness: HarnessStatus | undefined, // the run whose workdir === worktree.path
     hasAttachableSession: boolean,      // current_session present / session tab exists
   }) : 'starting' | 'session' | 'picker'
   ```

   - `pendingGatedRun && harness.state ∈ {driving/init} && !hasAttachableSession`
     → `'starting'` (render a "Gated run starting…" panel showing
     `harness.phase`/`current_feature`/`state`).
   - `hasAttachableSession` → `'session'` (clear the slice; normal session view).
   - else → `'picker'` (today's behavior; also the non-ownership fallback path,
     which never sets the slice).
3. Subscribe to `WS /api/harness/events` (existing `subscribeHarnessRunErrors`
   pattern / the harness event stream): `agent_spawned` for this run's
   `harness_id` → session now exists → flip to `'session'`; `harness_completed`
   success:false / `error` → clear slice + let the existing error toast fire.

**Matching a run to a worktree:** `HarnessStatus.workdir` is the absolute workdir;
compare via the same `normalizeWorkdir` used in `workspace-harness-offer.ts`
against `worktree.path`.

**Invariant checks:** no new poll (AC-4 — events only); the non-ownership fallback
(v0.84.1, `gated-run-ownership.ts`) and the `subscribeHarnessRunErrors` mid-spawn
toast are untouched (AC-3).

## Part B — unlink design

**Server (`crates/agentum-server`):**

1. **Primitive** (near the setter at `harness/types.rs:970-975`): a `clear_tracker`
   that sets `tracker_provider = None`/`tracker_url = None` on every feature and
   rewrites `feature_list.json` (pretty JSON, same as the setter). Unit-testable in
   isolation: after clear, `shared_tracker_provenance(&list)` (`types.rs:248`)
   returns `None`.
2. **Engine method** on `HarnessEngine`: look up the run by `id`, apply the
   primitive to the in-memory `FeatureList`, persist to disk under the run's
   `workdir`, emit `HarnessEvent::Log { harness_id, message: "issue unlinked" }`.
   Unknown id → `None`/error the route maps to 404.
3. **Route** (`harness.rs`): `POST /api/harness/{id}/unlink-issue` →
   `unlink_issue(State, Path(id))`; parse id (`parse_uuid`), call the engine
   method, 200 on success / 404 unknown. Not public (bearer token off-loopback).
4. **AC-6 falls out for free:** `apply_tracker_transition` (`drive.rs:392`) already
   early-returns when `feature.tracker_provider` is `None`, so once cleared no
   further transition posts to the old issue. No change needed there.

**Client (`harness-client.ts`):** `export function unlinkHarnessIssue(id: string):
Promise<void>` → `request('/api/harness/${id}/unlink-issue', { method: 'POST' })`
(mirrors `runHarness`).

**UI affordance:** the surface that displays a run's linked issue reads it from the
run's features. Pure helper `runLinkedIssue(status: HarnessStatus): string | null`
= `shared_tracker_provenance` mirror over `status.features.features` (first feature
with both `tracker_provider` && `tracker_url`). Where that chip renders (the
harness run panel / the worktree card meta — **NOT** the stale-cited
`HarnessEngine.tsx`, which has no tracker fields), add an "Unlink issue" button
that calls `unlinkHarnessIssue`, then optimistically clears the chip and/or waits
for the `log`/status refresh (no page reload — AC-7).

## Build order (Developer — on a FRESH origin/develop worktree)

Two independent features; recommended B-first (pure server + testable), then A:

1. **B-server**: `clear_tracker` primitive + `HarnessEngine` unlink method +
   `POST /api/harness/{id}/unlink-issue` + unit tests (clear →
   `shared_tracker_provenance` None; route 200/404). Gate: `cargo test -p
   agentum-server --lib`.
2. **B-client+UI**: `unlinkHarnessIssue` + `runLinkedIssue` pure helper (+ vitest)
   + the "Unlink issue" button on the run's issue surface. Gate: `npm run build`.
3. **A-pure**: `deriveGatedRunSurface` + the per-worktree "gated run starting"
   store slice, both pure (+ vitest).
4. **A-wiring**: set the slice in `maybeOfferWorkspaceHarnessRun` (ownership-gated),
   consume `deriveGatedRunSurface` in the workspace view, subscribe to harness
   events to flip 'starting'→'session'. Gate: `npm run build` + browser QA (qa.sh).

## Risks & invariants (architect sign-off)

- **One launch path (sacred):** Part A deliberately avoids a second spawn path and
  avoids mutating the harness `NewSession` (Q1). ✅
- **Push streaming, never poll:** Part A rides `WS /api/harness/events`. ✅
- **Sacred REPL mechanics (spec 008 D5):** untouched. ✅
- **Tracker best-effort:** unlink emits a `Log`, never halts a run; the AC-6 no-op
  is already the guarded behavior. ✅
- **Per-feature clear must hit ALL features** (the setter stamps all): the
  primitive iterates the whole `features` vec, asserted by the
  `shared_tracker_provenance`-is-None test. ✅
- **New route auth:** `/{id}/unlink-issue` is not added to `is_public` — token
  required off-loopback. ✅

## Handoff → Developer

- Blockers for autonomy: **stale worktree** (see top banner) — a human must
  provision a fresh `origin/develop` worktree before the Developer phase runs.
- All three open questions are resolved above; no PM send-back needed.
- Suggested feature_list.json (spec.md "Harness wiring") splits cleanly along the
  B-first build order.
