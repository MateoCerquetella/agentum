---
schema: 1
id: SPC-1B7GZZY3C8KXKZF1DKPRACVTBF
revision: 1
title: Gated-run surfacing on a new workspace + issue unlink
source: legacy-import:ai/specs/023-gated-run-surfacing-and-issue-unlink/spec.md@sha256:f5ec0b70afdc88148406cdf2bba2bfbb3a760c916b12c73f662c749f2e3ba60b
---

# Gated-run surfacing on a new workspace + issue unlink

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec 023 — Gated-run surfacing on a new workspace + issue unlink
>
> - **Number:** 023
> - **Status:** Architect         <!-- Draft | PM | Architect | In progress | Done -->  (PM + Architect complete; Developer ⛔ BLOCKED — needs a fresh origin/develop worktree, see architecture.md)
> - **Surface:** `crates/agentum-server` (harness routes/drive) + `crates/agentum-desktop/ui`
> - **Author:** Mateo Cerquetella
> - **Date:** 2026-07-17
> - **Tracker:** https://github.com/MateoCerquetella/agentum/issues/387
>
> > **Grounding note.** All `path:line` references below are against **`origin/develop`
> > (v0.86.0)**. The worktree this spec was drafted in
> > (`fix-start-gated-run-on-new-worrkspace-and-unlick`) is ~274 commits behind that
> > tip (based on v0.57.0), so implementation MUST happen on a fresh worktree cut
> > from `origin/develop` — do not build on this stale base (STALE-BASE merge trap).
> > The GitHub issue body was auto-generated against the stale tree, so its file
> > citations (`HarnessEngine.tsx`, `provision.rs`) are unreliable and are corrected
> > here.
>
> > **One-slice note (PM gate).** Per the user's explicit choice this is ONE combined
> > spec covering two independent asks from issue #387. The `validate_handoff.md`
> > "one slice" check will flag this; it is bundled deliberately because both trace
> > to the single ticket #387. The two parts (A: surfacing, B: unlink) are separable
> > and are wired as independent harness features so either can gate/ship alone.
>
> ## Problem
>
> When a user creates a brand-new workspace and immediately starts a **gated run**,
> the workspace lands on the empty "Start a session" agent picker — "multiple agents
> and nothing more" — with no sign that anything is starting. The gated run *is*
> running (an engine-spawned harness agent is booting in the background), but it is
> invisible: the user thinks the start silently failed and is left staring at a
> picker.
>
> Separately, once a harness run is attached to a GitHub issue, that association is
> permanent for the run's lifetime. If the issue was created by mistake, is the wrong
> one, or the user just wants to drive the run without tracker chatter, the only
> recourse today is deleting the entire run (`DELETE /api/harness/{id}`).
>
> ## Goal
>
> Make a freshly-created gated run **visible** in its own workspace (no more silent
> empty picker), and let a user **unlink** a run's tracker issue without destroying
> the run.
>
> ## Users / personas
>
> - **Mateo (power user / dogfooder)** creates a workspace with "Start gated run"
>   armed, then sits on the empty "Start a session" screen unsure whether the run
>   started — the moment part A fixes.
> - **Any driver** who filed the wrong GitHub issue (or wants to stop status
>   chatter mid-run) and does not want to nuke the whole run to detach it — the
>   moment part B fixes.
>
> ## Acceptance criteria
>
> ### Part A — surface the gated run in its new workspace
>
> 1. **Given** a workspace created with `gatedRun` armed and the engine took
>    ownership (`gatedRunResultOwnsWorktree` → true), **when** the workspace opens,
>    **then** the workspace view renders a visible "Gated run starting…" state (not
>    the bare `WorkspaceAgentLauncher` picker) that reflects the run's live
>    `HarnessState`/current-feature and clears once the engine-spawned session is
>    attachable.
> 2. The engine-spawned feature session is **associated with the created worktree**
>    so it can be surfaced in that workspace: `spawn_feature_agent`
>    (`crates/agentum-server/src/harness/drive.rs:442`) sets `worktree_path`/
>    `worktree_branch` on its `NewSession` (both are `None` today), OR the UI
>    surfaces the run by `harness_id`/workdir without needing the session to carry
>    the worktree. Whichever path is chosen, the new workspace shows the running
>    agent, not an empty picker.
> 3. The existing non-ownership fallback is preserved: when the engine did **not**
>    take ownership (`gatedRunResultOwnsWorktree` → false — start-work failed,
>    ineligible issue, or a zero-feature plan), the workspace still falls back to a
>    normal agent session (v0.84.1 behavior, `lib/gated-run-ownership.ts`), and a
>    loud toast still fires via `subscribeHarnessRunErrors` on a mid-spawn failure.
> 4. Part A adds **no** new polling loop: surfacing is driven by the existing
>    `WS /api/harness/events` stream / `HarnessState`, not a `capture-pane`/status
>    poll (push-based-streaming invariant).
>
> ### Part B — unlink a run's tracker issue
>
> 5. A new route on `crates/agentum-server/src/routes/harness.rs` clears the tracker
>    association for a run — `PATCH /api/harness/{id}` accepting `{ "issue_url": null }`
>    (added to the existing `.route("/api/harness/{id}", get(status).delete(stop))`
>    at `harness.rs:50`), or an equivalent `POST /api/harness/{id}/unlink-issue`.
>    It clears `tracker_provider`/`tracker_url` on every feature and persists
>    `feature_list.json`, so `shared_tracker_provenance` (`harness/types.rs:248`)
>    subsequently returns `None`.
> 6. After unlinking, `apply_tracker_transition` (`harness/drive.rs:392`, guarded by
>    `feature.tracker_provider`) becomes a silent no-op for that run, so subsequent
>    state transitions (`coding`/`verifying`/`done`/`blocked`) post **no** further
>    updates to the previously linked issue; the run otherwise continues normally
>    (features, gates, `handoff.md` unaffected).
> 7. The desktop UI exposes an **"Unlink issue"** affordance next to where the run's
>    linked issue is surfaced (the worktree card / `github-item-edit-section.tsx` /
>    the harness run panel — NOT the stale-cited `HarnessEngine.tsx`, which has no
>    tracker fields today). Clicking it calls the new endpoint via
>    `runtime/harness-client.ts` and reflects the cleared state **without a page
>    reload** (optimistic update or event-driven refresh).
> 8. Unlinking one run does **not** affect any other run's tracker association or
>    state.
>
> ### Gates
>
> 9. `cargo test -p agentum-server --lib` stays green, including new unit tests for
>    the unlink primitive (clear-then-`shared_tracker_provenance`-is-`None`) and the
>    route.
> 10. `npm run build --prefix crates/agentum-desktop/ui` completes without errors,
>     and the new pure UI decision (surfacing state / unlink call) has a vitest.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** surfacing a gated run in its own new workspace; associating the engine
>   session with that worktree (or an equivalent by-`harness_id` surface); a
>   clear-tracker route + persistence; a UI unlink affordance; unit + vitest gates.
> - **Out:**
>   - *Re-linking* to a different issue (unlink only; re-link is a follow-up).
>   - Changing `await_repl_ready` / `wait_for_settle` timing or the two-step
>     `inject_prompt` send sequence — the server-side "wait for pane ready before
>     injecting" contract the issue asks for **already exists**
>     (`drive.rs:991`/`1066`) and is sacred (spec 008 D5).
>   - Remote/SSH gated-run surfacing specifics beyond what the local path needs.
>   - Any change to the YOLO marker path or `spawn_agent_into_pane`.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `await_repl_ready` (`crates/agentum-server/src/harness/drive.rs:991`) +
>   `inject_prompt` (`drive.rs:1066`) — already wait for the workspace-trust dialog
>   and the idle REPL footer before typing, with a two-step submit. This is the
>   behavior issue #387's part-1 ACs describe; it is **not** rebuilt (invariant:
>   spec 008 D5 byte-identical send sequence).
> - `gated-run-ownership.ts` (`crates/agentum-desktop/ui/src/lib/`) +
>   `open-created-workspace.ts` (`planCreatedWorkspaceOpen`) — the v0.84.1 ownership
>   fallback and the three-skips gated-run suppression. Part A builds *on top* of
>   the ownership signal; it does not change the fallback.
> - `shared_tracker_provenance` (`harness/types.rs:248`) — resolves a run's tracker
>   from the first stamped feature. Unlink's success condition is "this returns
>   `None`".
> - The tracker-setter loop (`harness/types.rs:970-975`) that stamps
>   `tracker_provider`/`tracker_url` across all features and rewrites
>   `feature_list.json` — the unlink primitive is its inverse (set `None` + rewrite).
> - `apply_tracker_transition` (`harness/drive.rs:392`) — already a no-op when
>   `feature.tracker_provider` is `None`, so AC-6 falls out of AC-5 for free.
> - The harness route table (`harness.rs:41-55`) and `harness-client.ts` request
>   helper — the unlink route + client method slot into the existing pattern.
> - `subscribeHarnessRunErrors` (`harness-client.ts:382`) — the existing mid-spawn
>   loud-failure toast; preserved by AC-3.
>
> ### Build new
>
> - Part A: a workspace surfacing state ("Gated run starting…") keyed off the live
>   `HarnessState`/events for a just-created gated worktree; and the
>   session↔worktree association (set `worktree_path`/`worktree_branch` in
>   `spawn_feature_agent`, or a by-`harness_id` UI surface).
> - Part B: a `clear_tracker` primitive on `FeatureList`/the run (inverse of the
>   setter loop) + a `PATCH /api/harness/{id}` (or `/unlink-issue`) handler + a
>   `harness-client.ts` method + the UI "Unlink issue" affordance.
>
> ## Risks & invariants
>
> - **One launch path (sacred).** All agent spawns stay on
>   `routes::sessions::spawn_agent_into_pane`; part A must only add worktree fields
>   to the `NewSession`, never a second spawn path.
> - **Push-based streaming, never poll.** Part A surfacing rides
>   `WS /api/harness/events` / `HarnessState`; do not add a `capture-pane` poll
>   (AC-4).
> - **Sacred REPL mechanics (spec 008 D5).** Do not touch `await_repl_ready` poll
>   logic or the `inject_prompt` send sequence.
> - **Tracker best-effort.** A tracker/GitHub hiccup during unlink is logged
>   (`HarnessEvent::Log`), never halts the run.
> - **`worktree_path` side effects.** Setting `worktree_path` on the harness session
>   (AC-2 option 1) may change how existing surfaces (sidebar cards, worktree
>   session lists, teardown/reap) treat harness sessions — verify it does not make
>   the engine session user-killable in a way that breaks the drive loop. If risky,
>   prefer the by-`harness_id` UI surface instead.
> - **Unlink is per-feature, run-wide.** Clearing must hit *every* feature (the
>   setter stamps all), else `shared_tracker_provenance` still finds a stamped one
>   and AC-6 fails.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries** (independent, either can gate/ship alone):
>   - `A-surface-gated-run` — new workspace shows a live "Gated run starting…" state
>     for an owned gated run instead of the empty picker; engine session is
>     associated with / surfaced for the worktree.
>   - `B-unlink-issue` — clear-tracker route + persistence + UI affordance.
> - **`verify.sh` asserts (unit gate):**
>   - `cargo test -p agentum-server --lib` green, incl. new tests: unlink clears all
>     features → `shared_tracker_provenance` is `None`; the route accepts
>     `{issue_url: null}` and 404s an unknown id.
>   - `npm run build --prefix crates/agentum-desktop/ui` clean; new vitest for the
>     surfacing decision and the unlink client call.
> - **`qa.sh` asserts (browser QA gate):**
>   - Create a workspace with a gated run armed → the workspace shows a starting
>     state (not the empty "Start a session" picker) and the running agent appears
>     (screenshot evidence).
>   - On a run with a linked issue, click "Unlink issue" → the linked-issue chip
>     clears without a reload; a subsequent state transition posts nothing to the
>     issue.
>
> ## Open questions
>
> - **AC-2 approach:** set `worktree_path`/`worktree_branch` on the harness
>   `NewSession` (session becomes worktree-attached, simplest UI surface, but has
>   side effects on sidebar/teardown), **or** surface the run purely by
>   `harness_id`/workdir in the workspace view (no session-model change, more UI
>   wiring)? Architect to pick; leaning toward the by-`harness_id` surface to avoid
>   perturbing session/teardown semantics.
> - **AC-5 route shape:** `PATCH /api/harness/{id}` with `{issue_url: null}` (issue's
>   wording, general-purpose) vs a dedicated `POST /api/harness/{id}/unlink-issue`
>   (narrower, clearer intent). Recommend the dedicated POST unless a broader PATCH
>   is wanted for future run-field edits.
> - **Persistence of unlink across restart:** clearing `feature_list.json` persists
>   on disk, so the unlink survives a reload/restart — confirm that is the desired
>   semantics (vs. a session-only mute).
