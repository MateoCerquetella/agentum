# Handoff 02 — Architect → Developer (spec 009-wiki-project-scoped)

- **Date:** 2026-07-06
- **From:** sdd-architect (fresh subagent, autonomous run)
- **To:** sdd-developer
- **Spec:** `ai/specs/009-wiki-project-scoped/spec.md` (D1–D4 locked)
- **Architecture:** `ai/specs/009-wiki-project-scoped/architecture.md` (D-A1–D-A8)
- **Grounding:** `origin/develop` @ `388eaa66` (v0.58.3), all citations re-verified.

## Verdict

**Architect gate PASS.** Boundaries table, 8 decisions with tradeoffs, 5 risks
with mitigations, build order with per-slice gates and named tests. No open
questions gate implementation.

## Build order (strict): F1 → F2 → F3, one gated slice each

Follow `architecture.md` §4 exactly — it names every file and every test per
slice. Summary:

1. **F1 `projects-sidebar-wiki-off-rail`** — new `SidebarProjectsNav.tsx` +
   pure `projects-nav-rows.ts` (mounted in `sidebar/index.tsx:88–89`); the D1
   deletion inventory (D-A2, six numbered items); WikiPage collapses to
   embed-only (`pinnedRepoId` becomes **required**, name kept).
2. **F2 `wiki-quiet-probing`** — delete the sweep + `repoStatuses`; `wiki_keys`
   composite-key cache on AppState (positive-only, never cache the
   `path__<hash>` fallback, mutex never held across `.await`); fs.rs
   `is_click_to_open_dir_in` + dormant `prefetch` seam.
3. **F3 `wiki-push-status-progressive`** — `emit_wiki_updated` at the FOUR
   `write_status` sites (`:316`, `:330–338`, `:363–370`, `:372–380`) + the
   run-scoped `scan_pages_loop` in a `tokio::select!` with `wait_for_settle`;
   `Running` GET gains `pages`; new `wiki-view-state.ts` reducer; poll deleted
   with NO fallback (D-A5 — deviation from the spec's permissive letter,
   justified: embedded loopback means socket-down ⇒ HTTP-down too).

## Non-negotiables (from the architecture — regressions here fail review)

- **`vite build` does NOT typecheck** (`ui/package.json:8`). The D1 deletion
  and the poll removal MUST be pinned mechanically:
  `! grep -rn "openWikiPage\|closeWikiPage\|=== 'wiki'" crates/agentum-desktop/ui/src`
  (only `projectHubTab` hits allowed) and
  `! grep -n RUNNING_POLL_MS …/WikiPage.tsx`. Add both to verify.sh (or run
  them as part of each slice's gate).
- **Discriminator honesty:** UI flips to `ready` ONLY from a validated
  `GET /api/wiki`; a `wiki.updated{ready}` event is a refetch command, never a
  state flip. Pin with a `wiki-view-state.test.ts` case. 001 AC-9 Rust tests
  stay untouched and green.
- **Emit `ready` BEFORE `build_embeddings_sidecar`** (`routes/wiki.rs:360`),
  not after — embeddings are best-effort.
- **`projectHubTab`'s `'wiki'` entry stays** (`ui.ts:451`) — it's the hub tab,
  not the view.
- **Projects section ignores `filterRepoIds`** (D-A8) — comment the intent.
- **One launch path / YOLO / WorktreeList / ProjectHubPage untouched.**

## Gates per slice

`cargo test -p agentum-server --lib` && `npm run build --prefix
crates/agentum-desktop/ui` && `npx vitest run <touched paths>` (run from
`crates/agentum-desktop/ui`; there is no `npm test` script). Rust changes:
`cargo fmt --all` before committing (CI is fmt-gated). New UI logic goes in
pure modules (no jsdom in the UI package).

## Repo rules

- Work stays in this worktree (`wiki-remove-it-fomr-the-side` branch); commit
  per slice with clear messages; stage only your files (never `git add -A`).
- UI deps: `bun install` in `crates/agentum-desktop/ui` if node_modules is
  missing.
- Tests touching user paths isolate via `AGENTUM_HOME` (temp dir), not `XDG_*`.
