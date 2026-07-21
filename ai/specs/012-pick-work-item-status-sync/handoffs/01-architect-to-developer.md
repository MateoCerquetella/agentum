# Handoff 01 — Architect → Developer

- **Spec:** 012-pick-work-item-status-sync
- **Date:** 2026-07-08
- **From:** Architect (autonomous /sdd-loop iteration 1)
- **To:** Developer
- **Artifacts:** `architecture.md` (complete blueprint)

## Gate result

Architect gate: **PASS** — every AC has a design home (§3: F1 AC1–4, F2 AC5–7,
F3 AC8–10, F4 AC11–12); seams grounded in real code; 8 invariants numbered
(§2); tradeoffs + rejected alternatives stated (§10); per-slice first-failing
tests map to the gate (§8); the 5 open questions resolved with decisive defaults
+ carry-forwards flagged (§9, §11).

## ⚠️ Environment note (read FIRST — the #1 rule)

This worktree is **59 commits behind `origin/develop` and MISSING specs 009 +
010**. Spec 010 (v0.60.0) already shipped the Projects v2 binding
(`{project_id, status_field_id, status_mapping}`), the
`updateProjectV2ItemFieldValue` **Project-column write INSIDE
`apply_tracker_transition`**, the fuzzy option-ID discovery + nearest-earlier
fallback, and `done_closes_issue` — **none visible here.** Before writing ANY
Projects/binding code: **re-ground on fresh `origin/develop`** and confirm 010
didn't already build it. **Reuse-010-over-rebuild is invariant #1.** Every
`:line` in the blueprint is approximate — grep the named symbols.

## Build order + first move

F1 → F2 → F3 → F4, each an independently gated slice (`cargo test -p
agentum-server --lib` + `bun run build --prefix crates/agentum-desktop/ui` +
`bunx vitest run`; commit per green slice). **No `tsc` gate** (shared/* is a
vite alias).

**First failing test:** `work-item-picker-model.test.ts` →
`deriveIssueOptions excludes PRs and closed issues` (pure, jsdom-free).

## Non-negotiables (from the blueprint §2)

1. **Reuse 010, never rebuild** — add ONE `InReview` entry to 010's
   `status_mapping` + *call* `apply_tracker_transition`; write no Projects
   column code.
2. **One launch path** — InProgress trigger is a **bus subscriber**
   (`tracker_sync.rs` reactor), never inline gh in `spawn_agent_into_pane`;
   the spawn gains at most a pure broadcast, nothing that can throw.
3. **Idempotent · best-effort · never-halt** — every transition + every poller
   `gh` call logs on failure and never halts the session/gate/poll.
4. **Monotonic-forward `next_phase_write`** (pure) — `Todo<InProgress<InReview<
   ReadyToTest<Done`; blocks Done→InProgress on reopen; converges
   session-InProgress with harness-InProgress; poller Done is terminal +
   restart-safe (persisted `tracker_phase`).
5. **Fail-closed binding** — no `activeProject`/unparseable remote/empty Project
   ⇒ no bind, no transition; never a wrong-issue one.
6. **No webhooks → poll only** — `tracker_sync.rs` poller: bounded, backed-off,
   per-call timeout, per-tick cap; GitHub-only PR detection in v1.
7. **Registry serde-alias-FREE** — the 3 new `Worktree` fields
   (`tracker_provider`, `tracker_url`, `tracker_phase`) are
   `#[serde(default)] Option<String>` with **NO `#[serde(alias)]`**; a named
   test asserts an old-shape registry round-trips to `None` (not `[]`).
8. **`gh` behind the existing seam** — poller uses `task_sink`'s `gh_bin()` /
   fake-`gh` subprocess indirection (don't add a 4th `gh_bin` dup — recon #277).

## Developer confirmations to make on develop (blueprint §6, §9)

- Exact `apply_tracker_transition` signature + the `tracker_id` argument the
  arms need.
- Whether a clean session-"started" lifecycle broadcast already exists to
  subscribe to (§5) — if yes, subscribe; if not, add a one-line pure broadcast
  at the tail of `spawn_agent_into_pane`.
- Whether 010's Projects write accepts a pre-resolved Project item id and skips
  its resolve — if so, an optional `project_item_id` (serde-default, no alias)
  is a permitted optimization; otherwise omit it (default: omit).

## Key files (re-ground each on develop)

`crates/agentum-server/src/task_sink.rs` (`TrackerPhase`, `GithubStateMap`,
`apply_tracker_transition`, `gh_bin`), `linear.rs` (`LinearStateMap`),
`routes/worktrees.rs` (registry `Worktree` + create handler),
`routes/sessions.rs` (`spawn_agent_into_pane`), **new** `tracker_sync.rs`
(reactor + poller); `crates/agentum-desktop/ui/src/components/new-workspace/`
(`CreateWorkspaceWizard.tsx` step-3 Tracker, new `work-item-picker-model.ts`),
`hooks/useComposerState.ts` (`applyLinkedWorkItem`), `store/slices/worktrees.ts`
(`createWorktree`), `lib/launch-work-item-direct.ts` (shared bind payload),
`commands/gh_projects.rs` (`gh_get_project_view_table`, read path).

## Reviewer focus (carry forward)

010-reuse-not-rebuild · registry serde safety (old fixture → `None`, not `[]`) ·
one-launch-path (bus subscriber, no throw into launch) · best-effort/never-halt
(gh-nonzero test) · InReview 5-label set + independent option resolution ·
monotonic no-thrash + restart-safe terminal Done. Carry-forwards needing Mateo
(non-blocking): poll cadence 45 s; distinct "In Review" column vs fold; "any
agent session" as the InProgress trigger.
