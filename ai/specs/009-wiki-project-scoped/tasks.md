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

## F3 — `wiki-push-status-progressive` (AC 7–8) — DONE (this slice)

**What was done (developer, 2026-07-06):**

- `routes/wiki.rs`: route-local `emit_wiki_updated(&bus, repo_id, status,
  pages)` — `let _ = bus.send(Event::new("wiki.updated").with_payload(json!(…)))`
  (the `host.metrics` pattern), snake_case D4 payload
  `{ repo_id, status, pages? }`. Emitted at all four transition points:
  generate request path after the running `write_status` (`{running, pages:[]}`),
  inject-failure arm (`{failed}`), valid-index arm (`{ready, pages}` — slugs
  from the VALIDATED index, emitted BEFORE `build_embeddings_sidecar`),
  invalid/missing-index arms (`{failed}`). Broadcast-only, never persisted.
- Run-scoped `scan_pages_loop(bus, dir, repo_id, 2s)` raced against
  `wait_for_settle` in a `tokio::select!` (lifetime = the run's; the loop never
  returns). Growth gate extracted as pure `scan_grew(known, listed)` — emits
  only when a NEW slug appears (equality/shrink = silence).
- `list_page_slugs(dir)` (sorted `.md` stems, dotfiles skipped) shared by the
  scanner and the Running arm of `load_index_response` — `Running` gained
  `pages: Vec<String>` so a mid-run GET is progressive immediately. Added
  `rename_all_fields = "camelCase"` to `WikiIndexResponse`: enum-level
  `rename_all` only renames VARIANTS, so the variant fields (`session_id`,
  `schema_version`, `generated_at`) were silently snake_case on the wire —
  latent drift vs the TS type; now pinned camelCase by a wire-shape assertion
  in `running_response_lists_partial_pages`.
- `.status.json` semantics untouched; Ready still only via the validated index
  path (`parse_wiki_index` + `all_pages_present`). All five 001 AC-9
  loud-failure tests pass UNMODIFIED.
- NEW `ui/src/components/wiki/wiki-view-state.ts` (absorbs `wiki-probe.ts` —
  file + test deleted): `applyWikiEvent` can only merge pages into an EXISTING
  running state (monotone union — an early `pages:[]` frame can't contract the
  TOC) or command a `refetch`; `ready`/`failed` events NEVER flip state
  (D-A6). A `running` event when the view isn't running → refetch (the GET
  carries the authoritative `sessionId`). Plus `wikiProbePlan`, `prettifySlug`
  (kebab/underscore → Title Case), `commandForSocketOpen() → 'refetch'`.
- `WikiPage.tsx`: subscribes via `subscribeServerEvents` (`onEvent` reduces
  through `applyWikiEvent` over an `indexRef` mirror; `onOpen` → refetch per
  the bus contract); `RUNNING_POLL_MS` + the poll effect DELETED, no fallback
  (D-A5). `applyIndex` is the one owner of index transitions and clears the
  page cache on running→ready (mid-run partial fetches can't go stale-sticky).
  Progressive TOC: running-with-pages renders the two-pane layout behind a
  visible role="status" "Generating wiki…" banner, prettified-slug titles,
  pages clickable (page fetch allowed while running); running-with-zero-pages
  keeps the centered indicator.
- `runtime/wiki-client.ts`: Running variant gains `pages?: string[]`.
- NEW `wiki-view-state.test.ts` (14 cases): the discriminator pin
  (ready event ⇒ refetch, NOT a flip — reference-equal state), failed ⇒
  refetch, progressive merge + monotonicity + reference-equal silence,
  other-repo/other-kind/malformed frames inert, socket-reopen ⇒ refetch,
  probe-plan one-repo-only (folded), prettify.

**Gates run:** `cargo test -p agentum-server --lib` (571 = 569 base + 2 new,
0 failed; AC-9 tests listed green unmodified), `cargo fmt --all --check`
clean, `cargo clippy -p agentum-server --lib` no warnings,
`npm run build --prefix crates/agentum-desktop/ui` green,
`npx vitest run src/components/wiki` 14/14, pins: `RUNNING_POLL_MS` zero hits,
`wiki-probe` zero hits, `wiki-probe.ts` deleted — see the developer handoff.
