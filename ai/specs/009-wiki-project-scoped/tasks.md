# Tasks — Spec 009: Project-scoped Wiki

Build order is strict (architecture §4): F1 → F2 → F3, one gated slice each.

## F1 — `projects-sidebar-wiki-off-rail` (AC 1–3) — DONE (this slice)

**What was done (developer, 2026-07-06):**

- NEW `ui/src/components/sidebar/projects-nav-rows.ts` — pure row model
  `projectsNavRows(repos, activeView, activeRepoId)`; active requires
  `activeView === 'project'` AND matching `activeRepoId`.
- NEW `ui/src/components/sidebar/SidebarProjectsNav.tsx` — compact
  always-visible "Projects" group (native `<button>` rows, `RepoIconGlyph` +
  `displayName`, `aria-current` on active, `max-h-40` + `overflow-y-auto`,
  `onClick={() => openProjectHub(repo.id)}`); deliberately ignores
  `filterRepoIds` (D-A8, commented in-component). Hidden entirely when
  `s.repos` is empty.
- Mounted in `ui/src/components/sidebar/index.tsx` between `<SidebarNav />`
  and `<SidebarHeader />`.
- D1 deletion inventory (D-A2), all six items:
  1. `SidebarNav.tsx` — Wiki `PrimaryNavItem`, `openWikiPage` selector,
     `wikiActive`, `BookText` import.
  2. `store/slices/ui.ts` — `'wiki'` out of `activeView` + all
     `previousViewBefore*` unions; `previousViewBeforeWiki` member +
     initializer; `openWikiPage`/`closeWikiPage` types + implementations.
     `projectHubTab`'s `'wiki'` kept (hub tab).
  3. `App.tsx` — `activeView === 'wiki'` render arm + `WikiPage` lazy import.
  4. `hooks/resolve-zoom-target.ts` — `'wiki'` union entry.
  5. `SidebarNav.test.tsx` — inactive-items list `'wiki'` → `'projects'`.
  6. Sweep grep confirms zero stragglers (only `HubTab`/`projectHubTab`
     `'wiki'` hits remain — allowed).
- `WikiPage.tsx` collapsed to embed-only: `pinnedRepoId` now REQUIRED (name
  kept — `ProjectHubPage.tsx:181` unchanged); deleted the `selectedRepoId`
  fallback effect, the standalone header title branch, the rail render
  branch, `RepoRail`/`RailRepo`/`statusDot`/`repoName`, and the standalone
  "No projects yet" empty state. `sweep` reduced to the pinned-repo-only
  probe (NOT deleted — its full removal + `repoStatuses` is F2/AC-4).
- NEW `ui/src/components/sidebar/projects-nav-rows.test.ts` (5 cases).

**Gates run:** see the developer handoff — `bun install` (deps),
`npm run build --prefix crates/agentum-desktop/ui`,
`npx vitest run src/components/sidebar`, `cargo test -p agentum-server --lib`,
and the D1 grep sweep
(`grep -rn "openWikiPage\|closeWikiPage\|=== 'wiki'" ui/src` → only the
hub-tab hit at `ProjectHubPage.tsx:181`).

## F2 — `wiki-quiet-probing` (AC 4–6) — PENDING

- Delete `WikiPage.tsx` `sweep` + `repoStatuses` (+ the `RepoWikiStatus`
  type); NEW pure `wiki-probe.ts` (probe plan = exactly `[pinnedRepoId]`).
- `agentum-server` `AppState.wiki_keys` composite-key cache
  `(repo_id, path, host_id)`, positive-only (never cache the `path__<hash>`
  fallback), `std::sync::Mutex` never held across `.await`.
- `routes/fs.rs` `is_click_to_open_dir_in` + dormant `prefetch` seam + test.
- Gate: cargo lib tests (3 cache tests + fs guard test) + vite build +
  `npx vitest run src/components/wiki`. PR states the D3 audit result.

## F3 — `wiki-push-status-progressive` (AC 7–8) — PENDING

- `emit_wiki_updated` at the four `write_status` sites (`ready` emitted
  BEFORE `build_embeddings_sidecar`); run-scoped `scan_pages_loop` in a
  `tokio::select!` with `wait_for_settle`; `Running` GET gains `pages`.
- NEW `wiki-view-state.ts` reducer (absorbs `wiki-probe.ts`); WikiPage
  subscribes via `subscribeServerEvents` (`onOpen` → refetch); the 3 s
  `RUNNING_POLL_MS` poll deleted with NO fallback (D-A5).
- Discriminator honesty: `ready` only from a validated GET; a
  `wiki.updated{ready}` event is a refetch command — pinned by
  `wiki-view-state.test.ts`.
- Verify pins: `! grep -n RUNNING_POLL_MS …/WikiPage.tsx`; 001 AC-9 Rust
  tests untouched and green.
