# Review — Spec 014 live auto-status — SIGN-OFF (ship-ready)

- **Date:** 2026-07-10
- **Reviewer:** sdd-reviewer (autonomous /sdd-loop iteration 5)
- **Code:** `feat/014-live-auto-status` @ `d8bce1ba` (5 commits over develop
  v0.68.0), build worktree `agentum-014-live-auto-status`, tree clean.
- **Verdict:** **SIGN-OFF** — all 10 focus items PASS, no defects; 4
  leave-as-is nits; release conditions listed below are release-step
  obligations, not code send-backs.

## Focus items (all PASS, evidence quoted in the full report)

1. Emission under partial failure — label-ok+Projects-fail folds to `Skipped`
   (task_sink.rs:716–719); emit arm = single `matches!(Ok(Applied))`
   (:866/:1007); only two `bus.send` sites (the wrappers); blocked-Applied
   requires the label edit to succeed. Double-emission structurally impossible.
2. No behavior change to existing transitions — `transition_inner` = pre-014
   arms verbatim; `blocked_inner` delta = `with_comment` only; behavior-pinning
   tests intact; tester's 657/0/5 corroborates. (Reviewer had no git; triple-
   source agreement accepted.)
3. F4 worker safety — select!+MissedTickBehavior::Delay, Lagged→warn+continue,
   zero unwrap/expect on payloads, episodes keyed per-worktree, no lock held
   across awaits (due-set collected first), gh bounded by 30s timeout, isolated
   in its own spawned task.
4. Clear-verbatim cannot move the pipeline — persisted phase parsed verbatim,
   skip on None, `next_phase_write` never imported, never persists — rank-equal
   by construction; cannot fight guard or poller.
5. `with_comment` threading — harness passes `true` (pre-014 identical);
   suppression gates ONLY the comment; label + Projects blocked-column write
   unconditional. Pinned by fake-gh test.
6. Injection/log safety — all gh argv-exec (`Command::new().args()`), comment
   body one argv token, `format!` args never format strings, payloads
   `serde_json::json!`. Residual: ``` fence in a crash signature can break the
   comment's markdown cosmetically (pre-existing template, accepted).
7. UI discipline — pure models IO-free; malformed events → null (tested);
   slices no-op-on-equal; hooks unsubscribe + clear timers on unmount; one-hook
   ProjectViewWrapper hunk; events WS forwards `tracker.*` kind-agnostically.
8. Serde/registry — `struct Worktree` untouched (spec-012 shape); 3 additive
   camelCase JSON keys in `detected_row` only, null when unbound.
9. Spec fidelity — D1 (600s env, no focus-awareness), D2 (auto-clear, one
   comment/episode, 3600s named cooldown), D3 (F4 last, no F1–F3 imports on
   it) all honored; all 3 dev deviations verified strictly-safer or
   behavior-equivalent.
10. Release conditions — see below.

## Nits (leave-as-is)

- `begin_episode` stamps the comment budget before the gh write result is
  known — a failed write consumes the 1h budget (best-effort class; revisit if
  spec 015 touches attention: move the stamp behind `Ok(Applied)`).
- ms-scale TOCTOU between registry read and clear write vs a poller advance —
  self-heals on the next transition; not worth a lock.
- **Chip placement**: renders in the details-hover issue badge row (per the
  architecture's "beside IssueStateBadge" instruction) — visible on hover, not
  on the card face. If Mateo finds it buried, it's a one-line move. Live
  browser QA that would have validated the affordance was waived.
- Dev-handoff count slips (5/5 not 6/6 coalescer tests; 5905 not 5900 passed)
  — cosmetic, already flagged by tester.

## Release conditions (orchestrator MUST do all before tagging)

1. Merge fresh `origin/develop` (v0.68.1 `c096a596`) into the branch.
2. Post-merge re-gate: `cargo test -p agentum-server --lib` + `cargo fmt --all
   -- --check` (mandatory — `ec63dec8` touched harness/drive.rs, same file as
   014 F1/F4); recommended: `bun run build` + the two new vitest files.
3. Version bump v0.69.0 (Cargo.toml + Cargo.lock + tauri.conf.json).
4. Issue-first: labeled GitHub issue; PR into develop; `Closes #N` in the
   COMMIT MESSAGE (fires when it reaches main); FF-promote
   develop → staging → main; tag v0.69.0.
5. Ship notes record the waived/accepted items: live qa.sh (Mateo's waiver),
   bus-only events, in-memory ledger restart residual.

## Positive notes

`TrackerEmit` required-param makes "every caller emits" compiler-checked; pure
`Ledger(now: Instant)` gives real timing coverage without mocks; `wire_str` on
the seam type kills payload/persistence drift.
