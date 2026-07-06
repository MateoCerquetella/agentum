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

## F2 — `wiki-quiet-probing` (AC 4–6) — DONE (this slice)

**What was done (developer, 2026-07-06):**

- `WikiPage.tsx`: deleted the pinned-scoped `sweep` + its effect, the
  `repoStatuses` state and BOTH its writes (`refreshIndex`, `handleGenerate`),
  and the `RepoWikiStatus` type. The mount/repo-change effect now iterates
  NEW pure `wiki-probe.ts` (`wikiProbePlan(pinnedRepoId) → [pinnedRepoId]`),
  so a mount issues exactly ONE `GET /api/wiki` (AC-4). `getWiki` has exactly
  one caller left (`refreshIndex`), reached only from the plan-driven mount
  effect and the running-only poll (poll removal is F3). NEW
  `wiki-probe.test.ts` pins one-repo-only.
- F1 leftover: deleted the dead `setActiveView` selector in `SidebarNav.tsx`.
- `agentum-server/src/lib.rs`: `AppState.wiki_keys` —
  `Arc<Mutex<HashMap<(String, String, Uuid), String>>>` keyed
  `(repo_id, path, host_id)` with `LOCAL_HOST_ID` (nil UUID) for local
  (D-A3 doc comment on the field: TCC trigger, self-invalidation,
  positive-only). Initialized in `with_fingerprint` + every test-mod
  `fresh_state()` literal (board*.rs, sessions.rs ×2, mcp.rs, clipboard.rs).
- `routes/wiki.rs::resolve_target`: consults the cache before shelling
  `git remote get-url origin`; inserts ONLY on a successful non-empty remote
  (`should_cache_wiki_key(remote)`) — the `path__<hash>` fallback is never
  cached. Pure helpers `cached_wiki_key`/`insert_wiki_key` (lock → op → drop,
  never across an `.await`) + `WikiKeyCacheKey` alias. Tests (no git):
  `wiki_key_cache_hit_skips_resolution`,
  `wiki_key_cache_self_invalidates_on_path_or_host_change`,
  `wiki_key_cache_never_caches_path_fallback`.
- `routes/fs.rs` (D-A7, dormant by design): `is_click_to_open_dir_in`
  (macOS-gated, component-aware; ~/Desktop, ~/Documents, ~/Downloads,
  at-or-under any `/Volumes/<name>`; `/Volumes` itself NOT flagged) +
  `ListQuery.prefetch` (`#[serde(default)]`, false = explicit navigation,
  bit-identical). Both enforcement sites (`list_entries`, `list_dir`) keep
  the media bail unconditional and add `q.prefetch && is_click_to_open_dir`.
  Pointed comment at the seam: future automatic callers MUST set
  `prefetch=true`. Test `click_to_open_dirs_gate_prefetch_but_not_clicks`
  mirrors the media-dir test. **D3 audit (for the PR body): no automatic
  reads exist at base; the guard is dormant enforcement + regression tests.**

**Gates run:** `cargo test -p agentum-server --lib` (569 = 565 base + 4 new),
`cargo fmt --all --check`, `cargo clippy -p agentum-server --lib`,
`npm run build --prefix crates/agentum-desktop/ui`,
`npx vitest run src/components/wiki`, and the AC-4 sweep greps
(`getWiki(` only inside `refreshIndex`; `repoStatuses`/`RepoWikiStatus` →
zero hits) — see the developer handoff for outputs.

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
