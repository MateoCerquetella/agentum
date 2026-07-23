# Handoff 01 — PM → Architect

- **Spec:** 029-local-watchdog-fleet-scheduler
- **Date:** 2026-07-23
- **From:** PM (fresh serial SDD role)
- **To:** Architect
- **Artifact:** `ai/specs/029-local-watchdog-fleet-scheduler/spec.md`
- **Gate:** PASS

## PM gate evidence

| Gate item | Verdict |
| --- | --- |
| One slice | PASS — one operator-visible performance increment replaces local per-session sampling with one fleet schedule and its required batch transport; remote behavior is compatibility scope. |
| Problem before solution | PASS — the problem names N-scaling local timer/process/socket churn at 100 sessions. |
| Persona named | PASS — an engineer supervising dozens to hundreds of local agents, while also relying on remote SSH sessions. |
| Acceptance criteria testable | PASS — all eleven criteria use observable invocation counts, typed outcomes, events, durable state, deadlines, or explicit fake-runner/parser results. |
| Non-goals stated | PASS — SSH batching, streaming, adapters, schemas, routes, transcript/UI work, and a second watchdog loop are excluded. |
| Grounded in code | PASS — the spec cites the current watchdog reconcile/watch loop, cadence and classifiers, tmux combined sample, SSH sample path, adapter policy, and store/event seams. |
| Invariants respected | PASS — one scheduler, push streaming, exact targets/argv safety, adapter ownership, persistence-before-broadcast, and remote streaming-master behavior remain explicit. |
| Harness wiring present | PASS — three ordered feature entries plus focused verify/QA assertions cover transport, scheduler, compatibility, high-cardinality load, and source guards. |
| STATE transition | READY — the orchestrator owns the requested `pm → architect` update and decision-log append. |

No PM amendments were necessary: the draft already expresses the fixed intake as a bounded product contract without choosing low-level scheduler, parser, or runner design.

## Code-grounded findings

- `crates/agentum-watchdog/src/lib.rs` currently keeps `HashMap<Uuid, JoinHandle<()>>`, and `reconcile_once` spawns one `watch_session` task for each Running session. Each task sleeps on its own adaptive 1/2-second local or 3/6-second remote deadline and calls `agentum_tmux::ssh::sample_pane`.
- The existing per-session state machine owns crash-first processing, context-low cooldown and harness rotation, two-sample tool drift, then activity transitions. That order is the compatibility baseline in AC 7, not a new PM design.
- `crates/agentum-tmux/src/lib.rs::capture_pane_sample_combined` already demonstrates one argv-safe, exact-target tmux command sequence for a single local target. It is reuse evidence, not a sufficient fleet protocol: the current fixed separator and one-target parser do not satisfy nonce/pane attribution or 100-target batching.
- `crates/agentum-tmux/src/ssh/tmux_ops.rs::sample_pane` deliberately sends SSH sampling over `SshMux::Streaming` and distinguishes gone from transport/parser failure. AC 10 freezes that remote path while local sampling changes.
- The second healthy-cycle invocation is a ceiling, not a requirement. The architect may use it only where the fixed authoritative-Gone/pane-race contract requires it; routine healthy samples must remain within the same ≤2 bound.

## Fixed contract for architecture

The architecture must preserve every intake constraint together:

1. One local fleet scheduler replaces local per-session polling; no shadow local watch loop remains.
2. A healthy cycle for 100 due local sessions invokes local tmux at most twice and deduplicates shared targets.
3. Every requested target resolves to `Sample`, `Gone`, or fail-closed `Retry`, bound to a per-invocation nonce and observed pane identity.
4. Local context-low compaction is queued for the next batch, deduplicated, cooldown-limited, and never sent to harness-managed sessions.
5. Existing local 1/2-second and remote 3/6-second effective cadences, event payloads/order, crash semantics, tool/activity transitions, and initial delay remain compatible.
6. Removal, non-Running transitions, deletion, and retargeting invalidate stale samples and queued actions before they can mutate or emit.
7. Remote SSH behavior remains on the existing host-aware per-session path and streaming ControlMaster.
8. The fake runner/parser suite is authoritative for the complete matrix in AC 11, including the 100-session invocation ceiling.

## Architect decisions to pin

These are bounded implementation calls, not open product questions:

- Ownership and API boundary between the fleet scheduler and local tmux batch runner/parser.
- The fail-closed frame grammar and how pane identity is sampled strongly enough to reject pane replacement without trusting captured contents.
- Generation/deadline representation and the commit boundary that rejects stale results/actions after reconciliation.
- How the existing per-session classifier/event logic is extracted or reused once for accepted local samples while the remote path retains equivalent behavior.
- How the optional second local tmux invocation establishes authoritative `Gone` versus `Retry` without allowing one malformed target to poison unrelated well-framed results.

## Expected next artifact

Produce `architecture.md` with component boundaries, typed contracts, concurrency and stale-result rules, frame/parser safety, compatibility mapping, and an ordered build/test plan that covers AC 1–11. Then write `handoffs/02-architect-to-developer.md` on Architect PASS.
