# Verification Report — Spec 009 (Project-scoped Wiki) — sdd-tester

**Branch:** `wiki-remove-it-fomr-the-side` @ `2c3dc89d`, base `origin/develop` @ `388eaa66`
**Date:** 2026-07-06 · All gates re-run independently; no developer output trusted.

## SUMMARY VERDICT: **PASS-WITH-DEFERRALS**

All 9 ACs verified green in code + gates. AC-4 ruled **PASS-with-note** (qa.sh wording amendment required, see ruling). Four browser-visible probes deferred to qa.sh/staging with repro steps (spec-008 precedent). No blocking defects. Deviations audited and accurate. Sacred surfaces untouched.

---

## A. Gate re-run table

| # | Gate | Claimed | Independently observed | Verdict |
|---|------|---------|------------------------|---------|
| 1 | `cargo test -p agentum-server --lib` | 571/0/5 | **`571 passed; 0 failed; 5 ignored` (93.1s)** | PASS |
| 1a | `routes::wiki` tests | 13/13 | **13 passed / 0 failed**, incl. AC-9 loud-failure set: `index_running_then_failed_from_status_sidecar`, `index_ready_ignores_stale_running_sidecar`, `index_running_while_pages_still_writing`, `index_ready_round_trips_a_fixture`, `index_empty_when_nothing_generated` | PASS |
| 1b | AC-9 test fns unmodified | claimed | Diff proof: `grep -c "^-.*fn index_"` on the wiki.rs diff = **0**; the only tests-mod hunk is `@@ -756,6 +924,143` (6 context lines, **pure addition**); no hunk touches old lines 512–755 where the 001 tests live | PASS |
| 2 | `cargo fmt --all --check` | clean | **CLEAN** (exit 0) | PASS |
| 3 | `cargo clippy -p agentum-server --lib` | 0 warnings | **`Finished dev profile` — zero warning/error lines** | PASS |
| 4 | Vite build | green | **`✓ built in 1m 2s`** (chunk-size warnings pre-existing) | PASS |
| 5 | vitest sidebar+wiki | 31 baseline fails, new suites green | **`7 failed | 77 passed` files, `31 failed | 610 passed` tests.** Failing files are exactly the 7 pre-existing: `HostGroupHeader.test.tsx`, `StatusIndicator.test.ts`, `WorktreeCard.ssh-reconnect-prompt.test.tsx`, `WorktreeCardAgents.send-target.test.tsx`, `WorktreeCardAgents.test.tsx`, `WorktreeCardMeta.test.tsx`, `WorktreeList.lineage-child-card.test.ts` — **none touched by this branch.** Pristine baseline extract re-run: **identical 31 failed / same 7 files** (591→610 passed = +19 new/updated, all green). New/updated suites in isolation: **3 files / 26 tests, all pass** | PASS |
| 6 | Grep pins | all clean | `openWikiPage\|closeWikiPage\|=== 'wiki'` → **only** `ProjectHubPage.tsx:181` (allowed hub-tab hit); `previousViewBeforeWiki`, `repoStatuses\|RepoWikiStatus`, `RUNNING_POLL_MS`, `wiki-probe`, `RepoRail` → **all zero**. Own sweeps: `'wiki'` string appears only in `ProjectHubPage.tsx` + `ui.ts:450` (`projectHubTab` union — allowed); `BookText` remains only inside `WikiPage.tsx` (the embed's own icons); no `setInterval`/poll residue in WikiPage | PASS |

## B. Per-AC verdicts

| AC | Verdict | Evidence |
|----|---------|----------|
| **1** | **PASS** | Wiki `PrimaryNavItem` deleted (`SidebarNav.tsx` diff, replaced by a D1 comment); all six D-A2 deletions verified line-by-line in the diff (`ui.ts` unions/actions/initializer, `App.tsx:1752` arm + lazy import, `resolve-zoom-target.ts` union entry, `SidebarNav.test.tsx:17` → `'projects'`); sweep grep clean |
| **2** | **PASS** (visual deferred) | `SidebarProjectsNav.tsx` lists `s.repos` (hidden when empty), `onClick={() => openProjectHub(row.id)}`; `openProjectHub` verified: `setActiveRepo(repoId)` + `activeView: 'project'` (`ui.ts:872–885`) so the active-row predicate `activeView==='project' && activeRepoId===repo.id` is coherent; `projects-nav-rows.test.ts` 5/5 green; D-A8 filter-ignore commented in-component |
| **3** | **PASS** (visual deferred) | `ProjectHubPage.tsx:181` embed byte-identical (file untouched); probe effect iterates `wikiProbePlan(pinnedRepoId)` = `[pinnedRepoId]`, unit-pinned one-repo-only (2 tests) |
| **4** | **PASS-with-note** | Sweep + `repoStatuses` + `RepoWikiStatus` deleted (zero grep hits); `getWiki` has exactly one caller (`refreshIndex`). **See ruling in §C** for the two-GET mount count |
| **5** | **PASS** | `resolve_target` (wiki.rs:185–218) consults `cached_wiki_key` **before** the `git remote get-url` shell — the subprocess sits inside the `None` miss arm only; positive-only insert via `should_cache_wiki_key`; composite key `(repo_id, path, host.id)`; sync mutex, never across `.await`. 3 cache tests green, no git shelled in tests |
| **6** | **PASS** | Both enforcement sites (`fs.rs:201`, `:336`): `is_protected_media_dir(&resolved) || (q.prefetch && is_click_to_open_dir(&resolved))` — **media bail unconditional**, click-to-open gated on `prefetch` which is `#[serde(default)]` = false (explicit navigation bit-identical); no UI caller sets fs `prefetch`; `click_to_open_dirs_gate_prefetch_but_not_clicks` green; `/Volumes` itself not flagged, `~/Documents2` not flagged; D3 audit statement recorded in tasks.md for the PR body |
| **7** | **PASS** (live flow deferred) | Code-inspected all emission sites: after every one of the four `write_status` transitions — running (wiki.rs:432→435), inject-failure (:450→457), invalid-index (:495→502), missing-index (:506→513) — plus `ready` at :488 emitted **BEFORE** `build_embeddings_sidecar` at :492 with slugs from the **validated** index, plus scanner growth frames at :354. D4 payload snake_case `{repo_id, status, pages?}` (:322–326). Broadcast-only — zero `insert_event` hits. `RUNNING_POLL_MS` + poll effect deleted; NO fallback poll built (D-A5) |
| **8** | **PASS** (live growth deferred) | `load_index_response` Running arm carries `pages: list_page_slugs(dir)` (:681–684); `running_response_lists_partial_pages` green incl. the camelCase wire-shape pin; Ready still only via `parse_wiki_index` + `all_pages_present` (:666); discriminator pinned twice — Rust (mid-run stays Running) and vitest (`ready` event ⇒ refetch, **reference-equal** state); the five 001 AC-9 tests pass unmodified; `applyIndex` clears `pageCache` on running→ready (WikiPage.tsx:177–184) |
| **9** | **PASS** | All three gates re-run green (table §A) |

## C. The AC-4 ruling (mandatory): **PASS-with-note**

**Actual worst-case count (code-traced):** **2** same-repo `GET /api/wiki` on mount.
1. The probe-plan effect (WikiPage.tsx:211–224) → `refreshIndex(pinnedRepoId)` — 1 GET.
2. The subscription effect (:234–246): `subscribeServerEvents` calls `subscriber.onOpen?.()` **synchronously at subscribe time when the socket is already open** → `commandForSocketOpen()` = `'refetch'` → 1 more GET. If the socket is *not* yet open at mount, it's 1 GET now + 1 when the socket opens — same worst case, different timing.

No third path exists: `getWikiPage` hits `/api/wiki/{slug}` (a different read, only after a slug is active), and `handleGenerate` sets state from the POST response without a GET.

**Letter vs. intent:** AC-4's letter ("hub open now issues **exactly one** `/api/wiki` read") is violated in the worst case. Its intent — the entire Problem statement — is privacy: no every-repo sweep, no probe of a repo the user didn't open, no repeated `git remote get-url` subprocess (the TCC trigger). All of that holds: both GETs target the pinned repo the user explicitly opened; the second GET is a **cache hit** for any remoted repo (AC-5's cache — zero additional git subprocess); the remoteless-fallback residual re-runs one *local* git subprocess against the repo the user opened, which the spec explicitly accepts as user intent.

**Why not send back for a dedupe:** the onOpen refetch is the bus's documented contract and the *exact mechanism* D-A5 leans on to justify deleting the poll with no fallback (reconnect-gap heal). A mount-time dedupe guard would special-case mount vs. reconnect on socket timing and risk suppressing the one refetch that heals a genuinely stale snapshot after a real reconnect. The double-fetch is harmless by construction — `refreshIndex`'s `reqToken` guard makes the later response win. A dedupe buys one avoided loopback GET and costs a correctness edge. Not worth it.

**Required amendments (conditions of this PASS):**
1. **qa.sh wording** (spec.md §Harness wiring, `wiki-quiet-probing`): replace *"hub open issues exactly one `/api/wiki` read (no sweep, no other repo probed)"* with: *"with devtools network open, opening the hub Wiki tab issues `/api/wiki?repoId=<pinned>` reads **only for the pinned repo** (at most two on mount: the probe + the events-socket onOpen refetch) and **zero** reads for any other repo."* [APPLIED by orchestrator 2026-07-06]
2. **PR body** must note the AC-4 letter deviation with this rationale.
3. Informational for QA: React `StrictMode` is on (`main.tsx:45`) — a **dev** build double-invokes effects (up to 4 GETs); qa.sh must run against a production build, where 2 is the true worst case.

## D. Deviation audit — all four verified accurate, none breaks D1–D4 or architecture non-negotiables

- **F1 — standalone empty state deleted; dead `setActiveView` selector removed.** Accurate: WikiPage's `!repoId` branch is now a neutral "Loading…"; the removed `setActiveView` is only SidebarNav's unused *selector* — the store action itself survives (`ui.ts:890`). No D-A2 overreach.
- **F2 — `should_cache_wiki_key` positive-only; `WikiKeyCacheKey` alias in lib.rs; 8 test-literal touches.** Accurate: the 8 files each add exactly the one `wiki_keys:` line to `fresh_state()` — no route code touched (diff-verified). Matches the PM architect-note exactly.
- **F3 — `rename_all_fields = "camelCase"`.** Accurate and *correct*: a real wire change, but the TS type always declared camelCase — the old wire was latent drift (UI read `undefined`); pinned by the wire-shape assertion. **D4 is not violated**: D4 governs the *event* payload, which stays snake_case (verified at `emit_wiki_updated`). No cross-version concern (UI + server ship in one binary).
- **F3 — running-event-when-not-running ⇒ refetch; `scan_grew` name; union merge.** All verified in code + tests. The client-side **union** merge means an early/late `pages:[]` frame can never contract a GET-populated TOC (monotone, pinned by test). D-A5's no-fallback-poll is the architecture's own documented ruling (§5.1), honestly implemented.

## E. Sacred-surface check — all confirmed untouched

| Surface | Evidence |
|---|---|
| One launch path | `generate` still calls `routes::sessions::spawn_agent_into_pane` (wiki.rs:430); the diff hunk there adds only the emit call |
| YOLO marker | `flags: vec![agentum_executor::YOLO_MARKER.to_string()]` (wiki.rs:418), unchanged |
| `ProjectHubPage.tsx:181` embed | File not in the diff; grep hit at exactly `:181` |
| `projectHubTab` `'wiki'` union | Present at `ui.ts:450` |
| `is_public` / auth | `auth.rs` not in the diff; wiki router route-list **byte-identical** base vs HEAD (5 routes) — no new public surface |
| `.status.json` semantics | `write_status` unchanged; Ready reachable only via `parse_wiki_index` + `all_pages_present`; `WIKI_SETTLE_GRACE`/`TIMEOUT` unchanged |

## F. DEFERRED to qa.sh / staging (PASS(deferred) per spec-008 precedent) — repro steps

1. **Progressive TOC growth on a real generation:** open a local repo's hub → Wiki tab → Generate. Expect: full-pane "Generating wiki…" → as the agent writes `*.md`, the two-pane layout appears behind the `role="status"` banner with slug-Title-Case entries growing (≈2 s scanner lag), never shrinking → at green, real index titles replace slugs and a page opened mid-run re-renders final content (cache cleared).
2. **Loud failure path:** kill the generation agent mid-run (or corrupt `index.json` before settle). Expect: `wiki.updated{failed}` → view refetches → the recorded error text renders — never a half-empty Ready.
3. **Socket-reconnect heal (no poll anywhere):** while a wiki is Running, restart the embedded server / drop the events socket. Expect: on reconnect the view refetches once (onOpen) and shows current state; with the socket down, zero `/api/wiki` traffic.
4. **Two-hub event scoping:** two hubs pinned to repos A and B; generate in A. Expect: B's Wiki tab shows zero network traffic and zero state change for the whole run.
5. **AC-4 network assertion** (amended wording from §C): devtools network filter `api/wiki` — only pinned-repo reads, ≤2 on mount, zero for other repos. Production build (StrictMode note).
6. **TCC quiet-open** (the spec's raison d'être): with repos registered under `~/Documents`/a volume, open a *different* repo's hub — expect no macOS permission prompt for the unopened repos' locations.

## Defects found

**Blocking: none.**

- **INFO / ruling — AC-4 letter vs. behavior** (WikiPage.tsx:211–246): up to 2 same-repo GETs on mount. Ruled PASS-with-note; requires the qa.sh wording amendment + PR-body deviation note (§C). Not a code change.
- **COSMETIC (pre-flagged, D2-locked):** possible double "Projects" heading when `groupBy === 'repo'`. Verify visually in qa.sh slice 1; acceptable per D2.
- **INFO:** dev-build StrictMode double-invokes the mount effects (up to 4 GETs in dev only) — irrelevant to production/qa.sh, noted so a QA agent on a dev server doesn't misreport.
