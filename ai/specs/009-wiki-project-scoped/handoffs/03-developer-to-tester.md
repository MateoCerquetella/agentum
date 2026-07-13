# Handoff 03 — Developer → Tester (spec 009-wiki-project-scoped)

- **Date:** 2026-07-06
- **From:** sdd-developer (three fresh subagents, one per slice, autonomous run)
- **To:** sdd-tester
- **Commits (this worktree, branch `wiki-remove-it-fomr-the-side`, based on
  `origin/develop` @ `388eaa66`):**
  - F1 `b325c176` — Projects sidebar group; Wiki off the nav rail (embed-only)
  - F2 `8f1b663c` — quiet probing: sweep gone, wiki-key cache, dormant fs guard
  - F3 `fdfec986` — `wiki.updated` on the bus, poll removed, progressive render
- **All three slices code-complete.** `tasks.md` has per-slice details;
  deviations are documented there and in the decision log.

## Gate evidence (developer-run; re-run independently)

- `~/.cargo/bin/cargo test -p agentum-server --lib` → **571/0/5** (base 565 +
  4 F2 + 2 F3). The 001 AC-9 loud-failure tests pass **unmodified**
  (`index_running_then_failed_from_status_sidecar`,
  `index_ready_ignores_stale_running_sidecar`, etc. — 13/13 in routes::wiki).
- `cargo fmt --all --check` clean; `cargo clippy -p agentum-server --lib`
  zero warnings.
- `npm run build --prefix crates/agentum-desktop/ui` green (use
  `NODE_OPTIONS=--max-old-space-size=3072`).
- vitest: sidebar suites 12/12 (31 dir failures PROVEN pre-existing vs a
  pristine origin/develop extract — see scratchpad baseline); wiki suite
  14/14 (incl. the discriminator pin: a `wiki.updated{ready}` event is a
  refetch command, never a state flip).
- Grep pins all zero-hit: `openWikiPage|closeWikiPage|=== 'wiki'` (only the
  `projectHubTab === 'wiki'` hub-tab hit remains, allowed),
  `previousViewBeforeWiki`, `repoStatuses|RepoWikiStatus`, `RUNNING_POLL_MS`,
  `wiki-probe`.

## ⚠️ One ruling the tester must make (AC-4 letter vs. intent)

Mount of the hub Wiki tab can now issue up to **TWO** `GET /api/wiki` for the
**same pinned repo**: the AC-4 probe-plan effect + the events-bus contract's
mandatory onOpen refetch (fires at subscribe when the socket is already
open). The **one-repo-only** invariant (AC-4's rationale: no sweep, no other
repo probed, no git subprocess thanks to the F2 cache) holds; the F2 qa.sh
wording "exactly one `/api/wiki` read" will count two. Rule PASS-with-note
(recommended — amend the qa.sh wording to "reads only for the pinned repo")
or send back for a dedupe guard.

## Test per acceptance criterion (spec.md AC 1–9)

1. **AC-1** — no Wiki rail item; the grep sweep; `SidebarNav.test.tsx` green.
2. **AC-2** — Projects group lists `s.repos`; activating → `activeView==='project'`;
   `projects-nav-rows.test.ts` covers active-row logic. (It deliberately
   ignores `filterRepoIds` — D-A8, commented in-component.)
3. **AC-3** — hub Wiki tab probes the pinned repo only (see the ruling above).
4. **AC-4** — sweep deleted; `wiki-view-state.test.ts` folded probe-plan test.
5. **AC-5** — cache tests (hit / self-invalidate / never-cache-fallback);
   verify `resolve_target` consults before shelling git.
6. **AC-6** — `click_to_open_dirs_gate_prefetch_but_not_clicks`; explicit
   navigation is bit-identical (default `prefetch=false`); D3 audit statement
   is recorded in tasks.md for the PR body.
7. **AC-7** — `emit_wiki_updated` at 4 transitions + scanner; D4 payload
   shape; poll deleted, NO fallback (D-A5 justified deviation from the
   spec's permissive letter).
8. **AC-8** — `running_response_lists_partial_pages`; progressive merge +
   discriminator pin in vitest; AC-9(001) tests unmodified.
9. **AC-9** — the three gate commands above, all green.

## Live probes the developer could not do (browser/e2e — flag as deferred if not runnable)

- Generate on a real repo → banner + slug-titled TOC grows (2 s scanner,
  growth-only) → ready swaps real titles + clears the page cache (a page
  opened mid-run must re-render final content).
- Failure paths: garbled/missing `index.json` → `wiki.updated{failed}` →
  refetch shows the recorded error (loud).
- Socket-reconnect heal (onOpen refetch, no poll anywhere).
- Event scoping: two hubs on different repos — a run in repo A never touches
  repo B's view.
- Known-accepted: a page fetched mid-run renders partially-written content
  until ready (cache-cleared at ready).

## Notes

- Cargo lives at `~/.cargo/bin/cargo` (not on PATH).
- UI deps: `bun install` in `crates/agentum-desktop/ui` if node_modules missing.
- Reviewer flags carried forward: possible double-"Projects" heading when
  `groupBy === 'repo'` (D2 locked the design; cosmetic); verify.sh grep pins
  not yet wired into a `.harness/` scaffold (no scaffold exists for this spec
  — gates were run directly).
