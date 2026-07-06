# Handoff 01 — PM → Architect (spec 009-wiki-project-scoped)

- **Date:** 2026-07-06
- **From:** sdd-pm (fresh subagent, autonomous run)
- **To:** sdd-architect
- **Spec:** `ai/specs/009-wiki-project-scoped/spec.md` (Status: PM)
- **Grounding:** `origin/develop` @ `388eaa66` (v0.58.3); worktree
  `wiki-remove-it-fomr-the-side` is FF'd to it. ~15 citations line-verified by
  the PM at this commit.

## Verdict

**PASS — advance to architect.** All nine gate items pass; "one slice" is a
pass-with-note (3 ordered increments off ONE root cause — the every-repo
sweep; repo precedent 004/005/008). Six mechanical spec edits were required
and have been applied by the orchestrator (AC-1/AC-3/AC-4/AC-7 tightened,
qa.sh scoped per `$HARNESS_FEATURE_ID`, status header → PM).

## What the architect receives

Three gated increments, each independently valuable:

1. **F1 `projects-sidebar-wiki-off-rail`** (AC 1–3) — remove the Wiki rail
   item + delete the standalone wiki view (D1: full deletion incl. union
   entries, store actions, `App.tsx:1752` arm, `resolve-zoom-target.ts:14`,
   `SidebarNav.test.tsx:17`); add an always-visible Projects rail section
   (D2), rows → `openProjectHub(repo.id)`.
2. **F2 `wiki-quiet-probing`** (AC 4–6) — delete the every-repo `sweep`
   (`WikiPage.tsx:175–189`); in-memory repo→wiki-key cache in
   `routes/wiki.rs::resolve_target`; widen the fs protected-dir guard for
   automatic reads only (D3: likely lands as regression guard + tests — PR
   must state the audit result).
3. **F3 `wiki-push-status-progressive`** (AC 7–8) — emit `wiki.updated`
   (D4 payload: `{ repo_id, status, pages? }`) on the global `/api/events`
   bus from the generate task's transitions; WikiPage subscribes; 3 s poll
   removed (≥30 s fallback only while the socket is down); progressive page
   render during `running` without weakening the loud-failure contract.

## Locked decisions (constraints, not options)

D1–D4 in the spec's "PM decisions" section. Do not reopen them.

## Architect focus (PM's notes)

- **AC-5 cache invalidation:** prefer a self-invalidating cache key
  `(repo_id, path, connection_id)` over an invalidation callback — a stale
  key reads *another repo's wiki* (`wiki.rs:58–63` key semantics).
- **Page-write detection:** fs-notify vs server-local scan while `running` is
  your pick; server-local scanning does not violate push-not-poll (that
  invariant governs client↔server), as long as the client is event-driven.
- **AC-8 discriminator:** progressive TOC must never flip `.status.json`
  semantics — `ready` still requires validated `index.json` + all pages
  (001 AC-9 loud failure preserved; keep its regression test in verify.sh).
- **Invariants in play:** push-not-poll (F3 strengthens it — don't ship the
  event without removing the poll), one launch path (untouched,
  `wiki.rs:314`), YOLO translation (untouched).

## Expected artifact

`ai/specs/009-wiki-project-scoped/architecture.md` — boundaries (which crate/
module owns what), the event emission point(s) in `routes/wiki.rs:328–382`,
the cache design, the Projects-section component placement, tradeoffs +
risks, and a build order for F1→F3 with per-slice gates.
