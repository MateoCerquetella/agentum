# Handoff 03 — Developer → Tester (spec 014-live-auto-status)

- **Date:** 2026-07-09
- **From:** Developer (sdd-developer, autonomous /sdd-loop iteration 3)
- **To:** Tester
- **Verdict:** Developer gate **PASS**. All four slices code-complete, gated
  green, committed. Working tree clean.

## Where the code is

Worktree: `/Users/mateocerquetella/Developer/projects/agentum-014-live-auto-status`
Branch: `feat/014-live-auto-status` (based on origin/develop @ v0.68.0 `27c7c132`).
NOT pushed, no PR yet (release step comes after Reviewer sign-off; Mateo has
pre-authorized the release).

Commits (oldest first):
1. `2cbd9223` chore(fmt) — pre-existing fmt fallout isolated (cdp_screencast.rs, routes/chat.rs).
2. `90b7e804` F1 — `TrackerEmit{bus, worktree_id?}` required on both seam fns; `transition_inner`/`blocked_inner`; emit `tracker.phase_changed`/`tracker.blocked` on Ok(Applied) ONLY; `TrackerPhase::wire_str`; 6 call sites + poller/ensure_spec_and_plan bus threading.
3. `5b66a221` F2 — 3 camelCase tracker keys on `/api/worktrees/detected` rows (`detected_row` pure extraction); shared TS `Worktree` optional fields; NEW lib/tracker-phase.ts(+test), store/slices/tracker-phase.ts, hooks/useTrackerPhaseSync.ts, components/sidebar/TrackerPhaseChip.tsx; rendered beside IssueStateBadge in WorktreeCardMeta; mounted in App.tsx.
4. `b57265d4` F3 — pure coalescer (`PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS = 2_000`) + use-project-view-live-refresh.ts + ONE ProjectViewWrapper call (force refetch).
5. `d8bce1ba` F4 — NEW `tracker_attention.rs` (30s sweep, 3600s comment cooldown, `AGENTUM_ATTENTION_AFTER_SECS` default 600, pure Ledger/Fire, crash immediate, sustained-awaiting sweep, verbatim-phase clear); `with_comment: bool` through the blocked seam (harness passes true); spawned in lib.rs.

## Gate evidence (developer-reported; Tester must INDEPENDENTLY re-run)

- `cargo test -p agentum-server --lib` → 657 passed / 0 failed / 5 ignored
  (develop baseline 642). `cargo fmt --all` clean.
- `bun run build` (crates/agentum-desktop/ui) green.
- New vitest: tracker-phase.test.ts 12/12; project-view-live-refresh.test.ts 6/6.
- Full vitest: 39 failed files / 138 failed tests / 5900 passed — PRE-EXISTING
  baseline (matches the known ~38-file baseline; spot-check documented in the
  developer report). ZERO new failures claimed — verify this claim.

## Documented deviations (verify each against code)

1. `detected_row` pure extraction in routes/worktrees.rs — hermetic wire-shape
   test; JSON claimed byte-identical.
2. `resolve_bound_github` requires `tracker_provider == "github"` before any
   episode (spec scope: attention signal GitHub-only; also prevents spurious
   clear re-apply on linear binds).
3. `any_active_episode()` cheapness gate before recovery resolve (chatty
   `agent.working` frames touch no store/registry while nothing is flagged).

## Tester scope (per AC, cite evidence)

- AC 1–3 (F1): emission tests exist + pass; Skipped/Err emit nothing; existing
  transition behavior unchanged (TransitionResult values); no await on bus.
- AC 4–6 (F2): detected rows carry the 3 keys (null when unbound); pure model
  suite covers both event kinds, url-fallback matching, unbound⇒null chip,
  blocked⇒attention, phase_changed clears attention.
- AC 7 (F3): coalescer exactly-one-fetch inside 2 s window; no setInterval;
  unmount unsubscribes.
- AC 8–11 (F4): crash immediate blocked; sustained-awaiting threshold + one
  signal per episode; crash-loop cooldown ⇒ label re-applied, ONE comment;
  clear re-applies persisted phase verbatim (never fabricates/advances);
  never-halt on gh failure.
- Invariants: next_phase_write untouched; TrackerPhase = 5 variants; registry
  serde shape untouched (wipe test green); YOLO/launch/streaming untouched.
- DEFERRED (do NOT fail on these): live browser qa.sh scenario (waived by
  Mateo for this release); events-table persistence (by design, bus-only);
  in-memory ledger restart residual (accepted).

Verdict format: PASS / FAIL per AC with the test name or command output line
as evidence; any defect = FAIL with repro.
