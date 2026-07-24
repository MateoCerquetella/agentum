# Handoff 02 — Architect → Developer

- **Spec:** `029-local-watchdog-fleet-scheduler`
- **Date:** 2026-07-23
- **From:** Architect (fresh serial SDD role)
- **To:** Developer
- **Gate:** PASS
- **Artifacts:** `architecture.md`, `tasks.md`

## Gate evidence

| Gate item | Verdict |
| --- | --- |
| AC1–AC11 mapped | PASS — Architecture §5 gives an implementation seam and authoritative test for every criterion. |
| Ownership resolved | PASS — `agentum-tmux::local_batch` owns typed transport/framing/runner; `agentum-watchdog::fleet` owns registrations, dedupe, cadence, generation, actions, and effects. |
| Invocation bound proved by construction | PASS — one fleet probe plus at most one combined confirmation/action finish; no per-target tmux helper is callable from the batch loop. |
| Framing/races resolved | PASS — UUID-v4 invocation nonce, request id, bracketing `%pane_id`, strict closed frames, exact-target confirmation, and fail-closed Retry semantics. |
| Gone resolved | PASS — only a second exact-target absence with no prior pane identity becomes Gone; transport, partial output, or pane replacement is Retry. |
| Compaction resolved | PASS — crash inspection occurs between probe and finish; local compact is next-batch, generation-bound, target-deduped, cooldown-on-delivery, and harness-safe. |
| Stale boundary resolved | PASS — first probe is outside the state lock; generation filtering precedes finish; the commit guard spans actions and effects, so completed removal is final. |
| Behavior reuse resolved | PASS — one extracted SessionMachine retains crash → context → tool → activity order for both local and remote paths. |
| Remote preservation | PASS — remote registrations retain the existing per-session SSH sample/send path, streaming mux, and 3/6-second cadence. |
| Invariants | PASS — no shell interpolation, no schema/event/route/UI changes, no `pipe-pane` changes, and no duplicate local polling path. |

## Developer instructions

Implement `tasks.md` serially as F1 → F2 → F3. Do not leave the old local `watch_session` path
reachable beside the fleet scheduler, even temporarily at an F2 handoff. Keep the server-facing
`Watchdog` constructor, `with_running_sessions_hook`, and `reconcile_once` behavior stable.

The architectural hinge is the two-stage batch:

1. `probe` makes one nonce-framed tmux invocation for all unique due targets;
2. watchdog performs pure crash-first inspection and generation filtering;
3. `finish` makes zero or one invocation to confirm provisional absence and deliver only safe,
   identity-guarded pending compactions;
4. watchdog commits total Sample/Gone/Retry outcomes through the shared ordered machine.

Do not simplify `Gone` to stderr matching, use a fixed separator, send `/compact` in the probe, or
add an immediate local `send_keys` fallback. Each would break an explicit AC.

## Required proof before Tester handoff

- A recording subprocess fake proves exact command requests and one/two invocation bounds for 100
  targets.
- Raw parser fixtures prove delimiter collision, nonce mismatch, malformed/truncated/reordered
  records, and pane replacement cannot cross-attribute data.
- A paused-time recording scheduler proves initial/local/remote cadence and shared-target fanout.
- A barrier test retires/replaces registrations while probe is in flight and proves no stale
  mutation/event/action after removal completes.
- Ordered tests pin crash/compaction/tool/activity precedence and every current payload.
- Remote tests prove the unchanged streaming SSH path and 3/6-second cadence.
- All focused, harness, backend workspace, format, source-guard, and diff gates in `tasks.md` run.

Known environment limitations remain the same: full desktop workspace linkage may require the
missing Sherpa release dylib, and UI build requires installed Vite dependencies. This backend spec
must still run the non-desktop workspace gate and must record, not conceal, either external blocker.

There are no open questions and no human decision is required. Advance to Developer.
