# Handoff 02 — Architect → Developer (spec 014-live-auto-status)

- **Date:** 2026-07-09
- **From:** Architect (sdd-architect, autonomous /sdd-loop iteration 2)
- **To:** Developer
- **Verdict:** Architect gate **PASS**. Both delegated decisions are closed;
  `architecture.md` is the build blueprint — do not re-derive.

## Closed decisions (binding)

- **Q1** — emission lives **inside the seam**: `apply_tracker_transition` /
  `apply_blocked_transition` gain a required `TrackerEmit<'_>` param
  (`bus` + optional `worktree_id`); emit ONLY on `Ok(TransitionResult::Applied)`
  via `let _ = bus.send(...)`. Six call sites enumerated in architecture.md §1
  (compiler will find them all).
- **Q5** — **two distinct event kinds**: `tracker.phase_changed` (payload:
  worktree_id?, provider, phase wire-form, tracker_url?) and `tracker.blocked`
  (worktree_id?, provider, tracker_url, reason). Bus-only, no events-table
  persistence, no connect-replay.
- **F4 siting** — new `crates/agentum-server/src/tracker_attention.rs`
  (server crate; watchdog crate would be a dependency cycle). Sweep-tick
  worker (30 s granularity), pure `Ledger` decision core, per-WORKTREE
  episodes, `with_comment: bool` added to the blocked seam (harness passes
  `true`).

## Build order (one gated slice each — matches feature_list.json)

1. `tracker-phase-event` (F1) — seam wrapper + wire_str + 6 call sites +
   poller bus threading.
2. `phase-chip-live` (F2) — 3 JSON keys in `/api/worktrees/detected` rows +
   shared TS type + pure model/slice/hook/chip + WorktreeCardMeta render.
3. `board-live-refresh` (F3) — pure coalescer (2 s named constant) + hook +
   one ProjectViewWrapper call.
4. `attention-signal` (F4, LAST; demotable to 015 on red with zero rework).

Per-slice checkpoints + full test strategy: architecture.md §6–§7.

## CRITICAL build environment instructions

- **Branch off fresh `origin/develop`** — the loop's own worktree
  (`.claude/worktrees/how-can-we-make-it-auto-status`) is based on v0.57.0 and
  MUST NOT be used for implementation. Create a dedicated worktree:
  `git worktree add ../agentum-014-live-auto-status -b feat/014-live-auto-status origin/develop`
  (from the shared checkout path, without disturbing it — never
  checkout/reset/stash there).
- Fresh-worktree cargo gotcha (memory): `cargo check -p agentum-desktop` in a
  fresh worktree fails on `libsherpa-onnx-*.dylib` — copy both sherpa dylibs +
  onnxruntime from the main checkout's `target/release/` if you need the
  desktop crate; the spec's gate only needs
  `cargo test -p agentum-server --lib`, which avoids it. Use
  `$HOME/.cargo/bin/cargo`.
- UI: `bun install` first if node_modules is absent; verify with
  `bun run build` + `bunx vitest run` (bare `tsc` cannot resolve `shared/*`).
- Line numbers in spec/architecture were verified on `origin/develop`
  @ `8fb7eb16`; if develop moved, re-locate by function name (anchors are
  named in every citation).

## Gates the Developer must leave green

`cargo test -p agentum-server --lib` (incl. new F1/F4 emission + fake-gh
tests) AND `bun run build --prefix crates/agentum-desktop/ui` AND
`bunx vitest run` (new pure suites). `cargo fmt --all` before finishing.
Update `tasks.md` in the spec folder per slice; commit per repo rules (no
AI attribution trailers).

## Invariant checklist (verify before calling any slice done)

- `next_phase_write` diff EMPTY; no new `TrackerPhase` variant.
- Registry `Worktree` struct serde shape untouched (alias-free rule).
- No `setInterval`/poll added anywhere; F3 unsubscribes on unmount.
- No transition awaits the bus; `Skipped`/`Err` emit nothing.
- `spawn_agent_into_pane`, YOLO translation, pane streaming untouched.
