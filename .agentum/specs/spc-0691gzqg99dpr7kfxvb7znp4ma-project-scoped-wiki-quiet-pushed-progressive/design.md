# Architecture — Spec 009: Project-scoped Wiki (quiet, pushed, progressive)

- **Status:** Architect (spec status advances from PM)
- **Author:** sdd-architect (autonomous run)
- **Date:** 2026-07-06
- **Base:** `origin/develop` @ `388eaa66` (v0.58.3) — every citation below re-verified at this commit
- **Upstream:** `spec.md` (D1–D4 locked), `handoffs/01-pm-to-architect.md`

Citation corrections vs. the PM handoff (drift found while verifying):

- `resolve-zoom-target.ts` is at `crates/agentum-desktop/ui/src/hooks/resolve-zoom-target.ts` (not `lib/`); the `'wiki'` union entry is at `:14` as cited.
- `routes/wiki.rs` has **four** `write_status` sites, not two: `:316` (running), `:330–338` (inject failure), `:363–370` (invalid index), `:372–380` (no index). All four are `wiki.updated` emission points.
- `fs.rs` test: the `#[test]` fn is at `:631`, `#[cfg(target_os = "macos")]` attr at `:629`.
- `npm run build --prefix crates/agentum-desktop/ui` runs **`vite build` only** (`ui/package.json:8`) — esbuild transpilation, **no typechecking**. This materially affects how D1's deletion is gated (see Decision 2 and Risk 3).

---

## 1. Boundaries & ownership

| Piece | Owner (crate/module) | Nature of change |
| --- | --- | --- |
| Projects rail section | **NEW** `ui/src/components/sidebar/SidebarProjectsNav.tsx` + pure `ui/src/components/sidebar/projects-nav-rows.ts`; mounted from `ui/src/components/sidebar/index.tsx:88–89` | New, small, self-contained |
| Wiki rail item + standalone view deletion | `ui/src/components/sidebar/SidebarNav.tsx` (`:104`, `:195`, `:219`), `ui/src/store/slices/ui.ts` (`:440–447`, `:508–509`, `:872`, `:1079–1088`), `ui/src/App.tsx` (`:228`, `:1752`), `ui/src/hooks/resolve-zoom-target.ts` (`:14`), `ui/src/components/sidebar/SidebarNav.test.tsx` (`:17`) | Pure deletion (D1) |
| WikiPage embed-only collapse | `ui/src/components/wiki/WikiPage.tsx` — `pinnedRepoId` becomes **required** (kept name); `RepoRail`/`statusDot`/`repoName`/`RailRepo` (`:514–573`), the standalone header branch (`:356–366`), the selection-fallback effect (`:118–128`) deleted | Shrinks to the survivor |
| Wiki view state machine | **NEW** pure module `ui/src/components/wiki/wiki-view-state.ts` (F3; F2 adds the tiny `wiki-probe.ts` precursor) | New, vitest-covered |
| Event subscription | `WikiPage.tsx` consuming `subscribeServerEvents` from `ui/src/runtime/server-events-bus.ts:129` (the ONE shared `/api/events` socket); wire type in `ui/src/runtime/wiki-client.ts` | Consumer of existing bus |
| `wiki.updated` emission + page scanner | `crates/agentum-server/src/routes/wiki.rs` (generate's background task `:328–382`, plus the request path at `:316`) | Server, route-local |
| repo→wiki-key cache | `crates/agentum-server/src/lib.rs` (`AppState` field, precedent `stream_positions` at `:124–125`) + `routes/wiki.rs::resolve_target` (`:134–151`) | Server, one field + one fn |
| fs guard widening | `crates/agentum-server/src/routes/fs.rs` — new predicate beside `is_protected_media_dir_in` (`:124–131`), seam at the two enforcement sites (`:158–167`, `:278–287`), test mirroring `:631` | Server, regression guard (D3) |
| Event contract | `agentum-core::Event` (`lib.rs:429–438`) — **no change**; `kind` is an open dotted `String` | Untouched |
| `/api/events` transport | `routes/events.rs` — **no change**; new kind flows through `run`'s bus loop (`:99–118`) automatically | Untouched |

Not touched: `spawn_agent_into_pane` (one launch path, `wiki.rs:314`), YOLO translation, `WorktreeList.tsx` (its project-header rows at `:3049–3052`/`:3070–3085` are the behavioral precedent only), `ProjectHubPage.tsx` (already passes `pinnedRepoId` at `:181`).

---

## 2. Design decisions

### D-A1 — Projects rail section: own component, mounted as a sibling in `sidebar/index.tsx`

**Decision.** New `SidebarProjectsNav.tsx`, rendered in `components/sidebar/index.tsx` between `<SidebarNav />` and `<SidebarHeader />` (`:88–89`) — not inside `SidebarNav.tsx`.

- Reads `s.repos` directly (all of them — see D-A8 for the filter interaction) plus `s.activeView` / `s.activeRepoId` for the active row.
- Row = a native `<button>` (like `WikiPage`'s old `RepoRail` rows and `SidebarNav`'s items): `RepoIconGlyph` (or `FolderGit2`) + `repo.displayName`, `aria-current` when `activeView === 'project' && activeRepoId === repo.id`. Click **and** Enter/Space come free with `<button>` — no manual `onKeyDown` like `WorktreeList`'s div rows need.
- `onClick={() => openProjectHub(repo.id)}` — the exact call `WorktreeList.tsx:3050` already makes. No tab argument: `openProjectHub` (`ui.ts:876–889`) preserves the last `projectHubTab`.
- **No worktree rows, no status dots, no counts.** Worktrees are `WorktreeList`'s job; wiki status dots died with the sweep (F2) and re-adding them would reintroduce N× probing.
- Compact + bounded: a small uppercase "Projects" heading (the `RepoRail` heading style, `WikiPage.tsx:541–543`) and `max-h` + `overflow-y-auto` on the list, because `SidebarNav`/this section sit in the sidebar's fixed (non-scrolling) region above `WorktreeList` — ten repos must not push workspaces off-screen.
- Row-model logic extracted to pure `projects-nav-rows.ts` (`projectsNavRows(repos, activeView, activeRepoId) → {id, label, active}[]`) so it's vitest-testable without jsdom (repo convention).

**Tradeoff.** A sibling component keeps `SidebarNav` single-purpose (primary destinations) at the cost of one more file and a second store subscription. Mounting inside `SidebarNav` was the alternative; rejected because `SidebarNav` is `React.memo`'d around a stable prop-less contract and already 270 lines — and D2 explicitly allows "a small new component".

### D-A2 — D1 deletion inventory (exact), and WikiPage keeps `pinnedRepoId` as a **required** prop

**The deletion list (F1):**

1. `SidebarNav.tsx` — `:219` (rail item), `:104` (`openWikiPage` selector), `:195` (`wikiActive`), and the now-unused `BookText` import.
2. `store/slices/ui.ts` — `'wiki'` out of the `activeView` union (`:440`) and the six `previousViewBefore*` unions that carry it (`:441–445`, `:447`); delete the `previousViewBeforeWiki` member (`:446`), its initializer (`:872`), the `openWikiPage`/`closeWikiPage` type members (`:508–509`) and implementations (`:1079–1088`). `projectHubTab`'s `'wiki'` (`:451`) **stays** — that's the hub tab, not the view.
3. `App.tsx` — the render arm (`:1752`) and the lazy import (`:228`).
4. `hooks/resolve-zoom-target.ts` — `'wiki'` at `:14`.
5. `SidebarNav.test.tsx:17` — replace `'wiki'` in the inactive-items list (e.g. with `'projects'`).
6. Nothing else: grep confirms `openWikiPage`'s only caller is `SidebarNav.tsx:104/:219`, `closeWikiPage` has zero callers, `right-sidebar-visibility.ts` never lists `'wiki'`, and no palette/command references exist.

**WikiPage collapse (F1):** make `pinnedRepoId` **required** (keep the name — `ProjectHubPage.tsx:181` already passes it; the name still documents the embed contract; renaming to `repoId` is churn without value). That makes the following dead and deletable: the `selectedRepoId` state + fallback effect (`:117–128`, collapses to `pinnedRepoId` directly), `RepoRail` + `RailRepo` + `statusDot` + `repoName` (`:514–573`, `:74–76`), the standalone header title branch (`:356–366`), and the `pinnedRepoId ? … : …` branches (`:177`, `:418–425`). The every-repo arm of `sweep` (`:175–189`) becomes unreachable in F1 and is **deleted in F2** (AC-4 owns that assertion — keeps each slice's diff aligned with its AC).

**Critical gating note:** `npm run build` is `vite build` (no typecheck) and vitest also transpiles without typechecking — **deleting the union entries is not machine-enforced by the existing gates.** `verify.sh` must add a mechanical pin for AC-1, e.g. `! grep -rn "openWikiPage\|closeWikiPage\|=== 'wiki'" crates/agentum-desktop/ui/src`.

**Tradeoff.** Full deletion (vs. leaving `openWikiPage` as a hub redirect) removes any deep-link escape hatch; D1 locked this after verifying the redirect would be dead code by construction.

### D-A3 — `resolve_target` key cache: self-invalidating composite key on `AppState`, positive-only

**Decision.** Add to `AppState` (`agentum-server/src/lib.rs:97`):

```rust
/// repo→wiki-key cache: skips the per-call `git remote get-url` subprocess
/// (the TCC-prompt trigger and, over SSH, a network round trip). Keyed by
/// (repo_id, path, host_id) so a moved/re-homed repo self-invalidates.
pub wiki_keys: Arc<std::sync::Mutex<HashMap<(String, String, Uuid), String>>>,
```

`resolve_target` (`routes/wiki.rs:134–151`) computes the cheap parts every call (`resolve_repo_path` / `load_host_for_repo` — local `repos.json` + store reads, `repos.rs:339–370`), builds the composite key, and only on a miss shells `git remote get-url origin` (`:138`).

- **Self-invalidation, no callbacks:** if the repo's `path` or host changes, the composite key differs → miss → recompute. Stale entries become unreachable (lookups always use current path/host); no repo-mutation hook needed — exactly the PM's recommendation, since a stale key would read *another repo's wiki* (`wiki.rs:58–63`).
- **Positive-only caching:** cache **only** a successful, non-empty remote resolution (the `git__…` key). Never cache the `path__<hash>` fallback — over SSH a transport failure is indistinguishable from "no origin" in the current `.ok().filter(|o| o.success)` folding, and caching that would pin a repo to the wrong key until restart. Remoteless local repos re-run one cheap local subprocess per call, which is fine because F3 removes the 3 s poll (GETs become rare: mount + transitions).
- **Concurrency:** `std::sync::Mutex`, never held across an `.await` (lock → get/insert → drop) — the `stream_positions` precedent (`lib.rs:124–125`). Two concurrent misses both run git and insert the same value: benign.
- **No TTL, no eviction:** entries are ~100 bytes and repos number in the tens. Residual staleness class: a user re-points `origin` in-place (key change invisible to the composite) — stale until app restart; accepted, documented (see Risk 1).

**Test seam:** keep the cache read/insert as pure helpers over the map so `cargo test` covers hit / miss-on-path-change / never-caches-fallback without an `AppState` or a real git repo.

**Tradeoff.** Positive-only means the fallback path never benefits from the cache; accepted for correctness — a wrong wiki is much worse than a spare subprocess.

### D-A4 — `wiki.updated` emission: the `state.bus.send(Event::new(…))` pattern, four transition points + a run-scoped scan loop

**The bus pattern (named, existing):** `let _ = state.bus.send(Event::new("wiki.updated").with_payload(json!({…})));` — exactly `routes/host.rs:147` (`host.metrics`) and `routes/sessions.rs:301/:403/:419`. `AppState.bus` is `broadcast::Sender<Event>` (`lib.rs:99`); `Event.kind` is an open dotted `String` (`agentum-core/src/lib.rs:431`) so **no core change**. Payload keys snake_case per D4: `{ "repo_id", "status", "pages"? }`. Add one route-local helper `emit_wiki_updated(&bus, repo_id, status, pages: Option<&[String]>)`.

**Emission points in `routes/wiki.rs`:**

1. `generate` request path, right after `write_status(…, "running", …)` at `:316` → `{status:"running", pages:[]}`.
2. Background task, inject-failure arm (`:330–338`) → `{status:"failed"}`.
3. Background task, post-settle readback: valid index (`:353–361`) → `{status:"ready", pages}` emitted **after** the sidecar removal and **before** `build_embeddings_sidecar` (`:360`) — embeddings are best-effort and irrelevant to browse, and the GET is already Ready at that point; invalid index (`:363–370`) and no index (`:372–380`) → `{status:"failed"}`.
4. **Page-write scanner** while running (below) → `{status:"running", pages}` on growth.

**Page-write detection: server-local scan, not fs-notify.** The scanner is a small loop racing the settle wait inside the existing background task:

```rust
tokio::select! {
    _ = crate::harness::wait_for_settle(&st.bus, session.id, GRACE, TIMEOUT) => {},
    _ = scan_pages_loop(&st.bus, &dir_bg, &repo_id, Duration::from_secs(2)) => {},
}
```

`scan_pages_loop` lists `*.md` slugs in the wiki dir every ~2 s and emits **only when the slug set grows**. Justification vs. fs-notify: `notify` has precedent in this crate (`transcript_store.rs:33`), *but* the wiki dir is `remove_dir_all`'d + recreated at generate start (`wiki.rs:270–273`) — watching a deleted/recreated dir is a classic notify race; a watcher also needs lifecycle management (create on generate, tear down on settle/failure) where the scan loop's lifetime is structurally the run's lifetime (it dies with the `select!`). Cost: a local `read_dir` every 2 s for ≤20 min, only while a generation runs — zero idle cost. The PM already confirmed this is invariant-clean: push-not-poll governs client↔server, and the client stays event-driven.

**No events from `reindex`/`export`:** neither changes browse-visible state (`reindex` rebuilds the embeddings sidecar; `export` writes into the repo checkout). Not warranted — YAGNI.

**Not persisted, not replayed:** event persistence is opt-in per emitter (`store.insert_event`, e.g. `sessions.rs:738`) and the `/api/events` connect replay covers only `agent.*` snapshots (`events.rs:77–97`). `wiki.updated` stays broadcast-only; late subscribers get current state from the GET they already issue on mount. This is deliberate — see Risk 4.

**Tradeoff.** A 2 s scan can lag a page write by up to 2 s and coalesce two pages into one event. Irrelevant at the UX timescale (pages arrive tens of seconds apart) and it caps event volume by construction.

### D-A5 — UI subscription: `subscribeServerEvents`, poll deleted, **no** fallback poll

**Decision.** `WikiPage` (or a thin hook over the F3 reducer) subscribes via `subscribeServerEvents` (`runtime/server-events-bus.ts:129`) — the app's ONE shared `/api/events` socket. Consumer shape mirrors `HostLoadCards.tsx:62–68`:

- `onEvent`: ignore unless `ev.kind === 'wiki.updated'` and `ev.payload.repo_id === pinnedRepoId`. `status:"running"` + `pages` → progressive TOC update (pure reducer). `status:"ready" | "failed"` → **trigger `refreshIndex(repoId)`** — never construct a Ready state from the event (see D-A6).
- `onOpen`: `refreshIndex(repoId)`. This is the bus's documented contract (`server-events-bus.ts:21–31`): any snapshot a consumer depends on must be refetched on (re)connect. It fires immediately at subscribe time when the socket is already open, and on every reconnect.

**The 3 s poll (`RUNNING_POLL_MS` `WikiPage.tsx:54–55`, effect `:250–254`) is deleted, and the ≥30 s socket-down fallback the spec permits is NOT built.** Reasoning: the desktop talks to the **embedded loopback server** — if the events socket is down, HTTP is down too (same process), so a fallback poll would fail identically and adds a parallel path for zero coverage. The reconnect gap is fully healed by `onOpen`-refetch. This is the honest reading of AC-7 ("any retained poll" — none is retained) and it strengthens push-not-poll rather than diluting it. Pin the removal in `verify.sh` with `! grep -n "RUNNING_POLL_MS" crates/agentum-desktop/ui/src/components/wiki/WikiPage.tsx`.

**Tradeoff.** Against a hypothetical remote daemon where WS is blocked but HTTP works, the view would only refresh on remount. That deployment doesn't exist for this UI today; if it ever does, the fallback is a 10-line add behind the same reducer.

### D-A6 — Progressive render: extend the Running GET **and** carry event pages; discriminator stays honest

**Decision.** Both carriers, one truth:

- **Server:** `WikiIndexResponse::Running` gains `pages: Vec<String>` (slugs; camelCase on the wire like the rest of the GET). `load_index_response` (`:489–524`) populates it in the Running arm by listing `*.md` slugs — the same `list_page_slugs(dir)` helper the scanner uses. This makes hub-open-mid-run progressive **immediately**, without waiting for the next page event.
- **Event:** `pages?: string[]` (slugs) per locked D4 — used for live growth between GETs.
- **UI (`wiki-view-state.ts` reducer):** during `running`, render the TOC from the slug set behind the "generating…" banner; titles are prettified slugs (kebab → Title Case) until the validated index supplies real titles. Written pages are clickable (`GET /api/wiki/{slug}` reads the file regardless of state, `load_page` `:542–549`); `pageCache` is **cleared on the running→ready transition** so a partially-written page fetched mid-run can't survive as stale content.
- **Discriminator honesty (the load-bearing rule):** the UI state flips to `ready` **only** from a `GET /api/wiki` response — i.e. only via `load_index_response`'s validated path (`parse_wiki_index` `wiki.rs:115–137` + `all_pages_present` `routes/wiki.rs:530–540`). A `wiki.updated{status:"ready"}` event produces a *refetch command*, never a state flip — pinned by a reducer unit test. `.status.json` semantics are untouched; the 001 AC-9 tests (`routes/wiki.rs:624–651`, `:680–704`) keep passing unmodified.

**Tradeoff.** Slugs-only progressive titles are slightly ugly for a minute vs. reading each page's first heading (extra reads, another partial-file race). Locked D4 already chose `string[]`; prettified slugs are the simple, honest rendering of it.

### D-A7 — fs.rs guard widening (D3): predicate + seam, enforcement dormant by design

**Decision.** Per the PM's audit (no default/prefetch read of protected dirs exists today — the picker lists names without descending), AC-6 lands as a **regression guard**:

- **Predicate:** `is_click_to_open_dir_in(path: &Path, home: &Path) -> bool` beside `is_protected_media_dir_in` (`fs.rs:124–131`), same component-aware `starts_with` shape: true for `~/Desktop`, `~/Documents`, `~/Downloads` (dir or anything inside), and `/Volumes/<name>` (at-or-under any mounted volume root). macOS-gated like its sibling (`#[cfg(target_os = "macos")]` + a `false` fallback).
- **Seam:** `ListQuery` (`:230–242`) gains `#[serde(default)] prefetch: bool`. At the two existing enforcement sites (`list_entries` `:158–167`, `list_dir` `:278–287`): the media bail stays **unconditional**; the new check is `if q.prefetch && is_click_to_open_dir(&resolved) { return empty-listing }`. Default `false` = explicit navigation ⇒ today's behavior is bit-identical and workdir picking cannot break (the spec's over-block risk). Any *future* automatic/prefetch caller must set `prefetch=true` — enforced by convention + a pointed comment at the seam, which is the strongest guarantee available when no automatic caller exists to test against.
- **Test:** `click_to_open_dirs_gate_prefetch_but_not_clicks` mirroring `protected_media_dirs_are_flagged_but_projects_are_not` (`:631–660`): Desktop/Documents/Downloads/`/Volumes/NAS` flagged; `~/Developer/proj`, `~/Documents2`, `$HOME` itself not; plus the enforcement decision (prefetch+protected ⇒ empty, click+protected ⇒ lists).
- **PR statement (D3 contract):** the PR body must state the audit result explicitly — "no automatic reads exist at `388eaa66`; the guard is dormant enforcement + regression tests."

**Tradeoff.** Dormant enforcement is code nobody exercises in production yet; the alternatives are worse — no seam (a future prefetch regresses silently) or an `explicit` param on click paths (any missed call site breaks folder picking, the exact over-block the spec forbids).

### D-A8 — Projects section lists **all** repos, ignoring `filterRepoIds`

AC-2 says "every repo in `s.repos`", and the sidebar filter (`filterRepoIds`, `ui.ts:597`, persisted and re-validated at `:1528`) governs *workspace rows*, not project access — a repo you've filtered out of the workspace list must still be openable as a project (its hub is the only wiki path after F1). Comment this intent in the component. Do not wire `filterRepoIds` in without a PM change. (See Risk 5.)

---

## 3. Risks (top 5)

1. **Stale key cache reads another repo's wiki.** Highest-severity failure (silent wrong data). Mitigated by the self-invalidating composite key `(repo_id, path, host_id)` + positive-only caching (D-A3) — path moves, repo re-adds, and host changes all self-heal; transport-flaky SSH resolutions are never cached. **Residual:** re-pointing `origin` in-place keeps the old key until app restart — rare, documented in the code comment; escape hatch is a coarse TTL if it ever bites.
2. **Progressive render flips the discriminator (half-empty "success").** Mitigated structurally: `ready` is only reachable via `load_index_response`'s validated path; the event never carries enough to build a Ready state and the reducer turns `ready`/`failed` events into refetch commands (D-A6), pinned by `wiki-view-state.test.ts` ("event-ready must not flip state"); the 001 AC-9 Rust tests stay in `verify.sh` untouched.
3. **Deleting the `'wiki'` union breaks rehydrated UI state — verified: it can't, but the build won't catch stragglers.** `activeView` is **not persisted** (explicit comment + initializer `'activity'`, `ui.ts:864–866`); the `previousViewBefore*` fields are session-only initializers (`:867–873`); `hydratePersistedUI` never restores a view. The *real* residual risk is that `vite build` doesn't typecheck (D-A2), so a dangling `'wiki'` reference ships silently — mitigated by the `verify.sh` grep pin and by running the touched vitest suites (which at least execute the modules).
4. **Event flooding from page writes.** Bounded by construction: scanner emits only on slug-set growth at a 2 s cadence for a ≤20-min run (worst case ~600 tiny frames vs. the far chattier `agent.*` stream); `wiki.updated` is broadcast-only — never `insert_event`-persisted, never in the connect replay (`events.rs:77–97`) — and a lagged client degrades to the existing `bus.lagged` marker (`events.rs:109–115`) then self-heals via `onOpen`-refetch.
5. **Projects section vs. sidebar filter/grouping confusion.** A repo hidden by `filterRepoIds` still shows under Projects (AC-2, D-A8) — users may read that as the filter "not working"; and a long repo list could crowd the fixed rail region. Mitigations: one-line compact rows with a bounded `max-h` + scroll, the section is visually a distinct labeled group (not workspace rows), intent documented in-component; D2 already de-risked the grouping axis by making the section independent of `groupBy` (`SidebarHeader.tsx:17–19` keeps its title behavior untouched).

---

## 4. Build order — three gated slices

Convention reminder for every slice: interactive UI logic goes into **pure modules** for vitest (the UI package has no jsdom; components themselves are exercised only by `vite build`). There is no `npm test` script — run `npx vitest run <paths>` from `crates/agentum-desktop/ui`.

### F1 — `projects-sidebar-wiki-off-rail` (AC 1–3)

**Files:**
- NEW `ui/src/components/sidebar/SidebarProjectsNav.tsx`, NEW `ui/src/components/sidebar/projects-nav-rows.ts`
- `ui/src/components/sidebar/index.tsx` (mount between `:88` and `:89`)
- `ui/src/components/sidebar/SidebarNav.tsx` (delete `:104`, `:195`, `:219`, `BookText` import)
- `ui/src/store/slices/ui.ts` (unions `:440–447`, members `:446`/`:508–509`, initializer `:872`, actions `:1079–1088`)
- `ui/src/App.tsx` (`:228`, `:1752`)
- `ui/src/hooks/resolve-zoom-target.ts` (`:14`)
- `ui/src/components/sidebar/SidebarNav.test.tsx` (`:17`)
- `ui/src/components/wiki/WikiPage.tsx` (prop required; delete `:117–128` fallback, `:356–366` title branch, `:418–425` rail branch, `:514–573` RepoRail block, `:74–76` `repoName`)

**Tests:**
- `projects-nav-rows.test.ts` — rows from repos; active row requires `activeView==='project'` AND matching `activeRepoId`; empty repos ⇒ empty rows.
- Updated `SidebarNav.test.tsx` (the `:17` list).
- Sweep check: `grep -rn "'wiki'\|openWikiPage\|closeWikiPage" ui/src` returns only `projectHubTab`-related hits (this doubles as the AC-1 verify.sh pin).

**Gate:** `npm run build --prefix crates/agentum-desktop/ui` && `npx vitest run src/components/sidebar` && `cargo test -p agentum-server --lib` (untouched but keeps the harness gate uniform).

### F2 — `wiki-quiet-probing` (AC 4–6)

**Files:**
- `ui/src/components/wiki/WikiPage.tsx` — delete `sweep` (`:175–189` at base; now the unreachable every-repo arm) + `repoStatuses` state + its writes (`:220`, `:303`); NEW tiny pure `ui/src/components/wiki/wiki-probe.ts` (probe plan: exactly `[pinnedRepoId]`).
- `crates/agentum-server/src/lib.rs` — `wiki_keys` AppState field (+ construction sites).
- `crates/agentum-server/src/routes/wiki.rs` — `resolve_target` cache consult/populate + pure cache helpers.
- `crates/agentum-server/src/routes/fs.rs` — `is_click_to_open_dir_in` + `prefetch` seam + comment.

**Tests:**
- Rust: `wiki_key_cache_hit_skips_resolution`, `wiki_key_cache_self_invalidates_on_path_or_host_change`, `wiki_key_cache_never_caches_path_fallback` (pure helpers, no git needed).
- Rust: `click_to_open_dirs_gate_prefetch_but_not_clicks` (mirrors `fs.rs:631`).
- vitest: `wiki-probe.test.ts` — one-repo-only (the AC-4 assertion).

**Gate:** `cargo test -p agentum-server --lib` && `npm run build --prefix crates/agentum-desktop/ui` && `npx vitest run src/components/wiki`. PR body states the D3 audit result.

### F3 — `wiki-push-status-progressive` (AC 7–8)

**Files:**
- `crates/agentum-server/src/routes/wiki.rs` — `emit_wiki_updated` helper; emissions at `:316`, `:330–338`, `:353–361`, `:363–370`, `:372–380`; `scan_pages_loop` + `tokio::select!` around `wait_for_settle` (`:340–346`); `list_page_slugs(dir)` shared with the Running arm of `load_index_response` (`Running` gains `pages`).
- `ui/src/runtime/wiki-client.ts` — Running variant `pages?: string[]` (`:27`).
- NEW `ui/src/components/wiki/wiki-view-state.ts` (reducer; absorbs F2's `wiki-probe.ts`).
- `ui/src/components/wiki/WikiPage.tsx` — subscribe (`onEvent`/`onOpen` per D-A5), delete `RUNNING_POLL_MS` (`:54–55`) + poll effect (`:250–254`), progressive TOC + banner, `pageCache` clear on running→ready.

**Tests:**
- Rust: `running_response_lists_partial_pages` (`.status.json` running + some `*.md` ⇒ `Running{pages}`); `scan_diff_emits_only_on_growth` (pure slug-set diff); existing AC-9 loud-failure tests unchanged and green.
- vitest `wiki-view-state.test.ts`: running event merges pages progressively; **ready event ⇒ refetch command, not a state flip** (the discriminator pin); failed event ⇒ refetch; other-repo events ignored; socket-reopen ⇒ refetch.
- verify.sh pins: `! grep -n RUNNING_POLL_MS …/WikiPage.tsx`.

**Gate:** `cargo test -p agentum-server --lib` && `npm run build --prefix crates/agentum-desktop/ui` && `npx vitest run src/components/wiki`. qa.sh (per `$HARNESS_FEATURE_ID`) covers the browser-visible ACs as speced.

---

## 5. Simplification review (honest cuts)

1. **The ≥30 s fallback poll: cut entirely** (D-A5). The spec permits retaining it; against an embedded loopback server it is a parallel path with zero coverage. This is the one place I'd push back on the spec's letter in favor of its own invariant.
2. **Progressive titles: prettified slugs, nothing more.** Don't read page files for headings during `running` — D4's `pages: string[]` already decided this; resist "just parse the first line".
3. **No events for `reindex`/`export`** — not browse-visible. If someone asks for an export toast, the HTTP response already carries it.
4. **Cache: no TTL, no eviction hooks, no repos-route invalidation calls.** The composite key does the work; every added mechanism is a new staleness bug surface.
5. **fs.rs: don't wire `explicit`/`prefetch` flags into any UI call site.** No automatic reader exists; the dormant seam + tests are the whole deliverable (D3 expected exactly this).
6. **Don't add wiki-status dots to the Projects section.** They'd require per-repo probes — reintroducing the sweep this spec exists to kill. If status-at-a-glance returns someday, it rides `wiki.updated` into a store slice, not GET probes.
7. **`wiki-view-state.ts` is the one new abstraction F3 buys** — accept it; it's what makes the poll-removal and discriminator rules testable without jsdom. Everything else in F3 is wiring.

---

## Handoff → Developer (sdd-developer)

- **Completed:** architecture validated against `388eaa66`; all PM citations re-verified (corrections listed at top); cache/event/scanner/guard designs grounded in existing patterns (`stream_positions`, `host.metrics` bus send, `transcript_store` notify precedent considered and rejected for this use, `HostLoadCards` subscription shape).
- **Pending:** implementation F1 → F2 → F3, strictly in order, one gated slice each (Section 4).
- **Key decisions:** D-A1–D-A8 above; D1–D4 remain locked from PM.
- **Watch out for:** `vite build` does not typecheck — the D1 deletion and the poll removal need the grep pins in `verify.sh`; never hold the `wiki_keys` mutex across an `.await`; emit `ready` before `build_embeddings_sidecar`, not after.
- **Open questions (non-blocking):** none that gate implementation. If Mateo wants wiki-status dots in the Projects section later, that's a new spec riding `wiki.updated`.
- **Recommended next step:** sdd-developer implements F1 under the harness gate; sdd-reviewer checks the D1 grep sweep and the discriminator reducer test before F3 merges.
