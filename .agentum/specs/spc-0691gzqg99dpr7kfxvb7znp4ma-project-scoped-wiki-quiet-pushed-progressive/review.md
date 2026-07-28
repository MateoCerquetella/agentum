# Review — Spec 009 (Project-scoped Wiki) — sdd-reviewer

**Branch** `wiki-remove-it-fomr-the-side` @ `673c6ecf` vs `origin/develop` @ `388eaa66` · 2026-07-06
**Inputs:** handoff 04, verification.md (gates trusted as independently re-run), spec.md (D1–D4 + applied qa.sh amendment), architecture.md (D-A1–D-A8), and direct inspection of every new/changed surface.

## VERDICT: **SIGN-OFF → SHIP-READY**

No blocking defects. The AC-4 tester ruling is **upheld**. The double-"Projects" heading is **leave-as-is** (D2-locked; qa.sh will see it). Two non-blocking should-fix items below are follow-up-ticket material, not send-backs.

---

## Focus-item table

| # | Item | Verdict | Evidence |
|---|------|---------|----------|
| 1 | **Discriminator honesty end-to-end** | **PASS** | No path constructs Ready from an event. `wiki-view-state.ts:83–87`: ready/failed events return `{ index: current, command: 'refetch' }`; the running arm merges only into an *existing* running state (`:89–93` refetches otherwise). `WikiPage.tsx:177–184` — `applyIndex` is the single transition owner; the subscription (`:237–241`) only applies `outcome.index` and turns `'refetch'` into a GET. The only non-GET `applyIndex` call constructs **Running** from the generate POST response (`:309`) — allowed (server response, not Ready). Rust: `load_index_response` (wiki.rs:653–692) reaches Ready solely via `parse_wiki_index` + `all_pages_present`. Vitest pin asserts reference equality (`wiki-view-state.test.ts:48`). |
| 2 | **Cache correctness knife-edge** | **PASS — residual is exactly the accepted class** | Adversarial: (a) repo moved → path differs → miss; (b) repo removed+re-added → `repos.rs:140` mints a new UUID → miss; (c) host changed → miss; (d) SSH flake → `should_cache_wiki_key` false → fallback served once (pre-existing base behavior), never cached; (e) concurrent misses → same value inserted twice, benign, documented; (f) mutex poison → `lock().ok()` degrades to permanent miss, never wrong data. Only wrong-wiki path: origin re-pointed in place under unchanged `(repo_id, path, host_id)` — the documented restart-bounded residual (lib.rs:139–143). Lock never held across `.await`. |
| 3 | **Scanner lifetime** | **PASS** | Inject failure returns *before* the `select!` (wiki.rs:449–459) — scanner never constructed. Settle timeout completes at 1200s → `select!` drops the scanner (`:464–472`). `scan_pages_loop` has no panic paths (`list_page_slugs` swallows IO errors); a panic in `wait_for_settle` kills the whole spawned task — no orphan possible (orphaned running sidecar = pre-existing 001 risk class, covered by `index_ready_ignores_stale_running_sidecar`). Event storm bounded: growth-only gate + 2s cadence; shrink-then-regrow re-emits full listing which the client union-merges. |
| 4 | **AC-4 ruling** | **UPHELD (PASS-with-note)** | Independently verified: `server-events-bus.ts:131–132` fires `onOpen` synchronously at subscribe when the socket is open → mount = probe GET + onOpen GET, worst case 2, both pinned-repo. The onOpen refetch is the bus contract and the exact mechanism D-A5's poll deletion leans on for reconnect heal — a dedupe trades a user-invisible loopback GET for a subtle timing dependency; "not worth it" is right. Second GET = cache hit for remoted repos (zero git subprocess). qa.sh amendment confirmed applied (spec.md §Harness wiring). Conditions carry to the PR body. |
| 5 | **D1 deletion completeness (adversarial)** | **PASS** | Beyond the tester's greps: case-insensitive `wiki` sweep — only unrelated hits (`feature-wall-tiles.ts:194`, `monaco-markdown-doc-link-decorations.ts:115`, both about `[[wiki-link]]` markdown copy); all `setActiveView` callers use literals (no computed view strings); `activeView` not persisted and `hydratePersistedUI` (`ui.ts:1435–1580`) never restores a view; zero `'wiki'` in palette/shortcuts outside the hub-tab union; `openWikiPage`/`closeWikiPage`/`previousViewBeforeWiki`/`wikiActive`/sidebar-`BookText`/`wiki-probe.ts` all zero. |
| 6 | **Quality/maintainability** | **PASS** | Comments say *why* (`SidebarProjectsNav.tsx:17–24` D-A8 rationale; `ListQuery.prefetch` doc fs.rs:283–293; `lib.rs:134–144` cache policy at the field). Pure-module extraction matches the no-jsdom convention, both well-tested (reference-equality pins, malformed-frame cases). No F1/F2 orphans (`wiki-probe.ts` genuinely absorbed — 3 files in components/wiki/). Rust helpers mirror named precedents (`stream_positions`, `host.metrics` emit). Two minor should-fixes below. |
| 7 | **D1–D4 + architecture non-negotiables** | **PASS** | D1 fully executed; D2 separate always-visible section, native `<button>` rows; D3 media bail unconditional + dormant default-false `prefetch` seam (fs.rs:201, :336) + audit statement recorded (tasks.md:82–83); D4 `emit_wiki_updated` snake_case `{repo_id,status,pages?}` (wiki.rs:322–326), one kind. No fallback poll anywhere; one launch path (wiki.rs:430, only the emit added); YOLO `:418`; ready emitted before `build_embeddings_sidecar` (`:488` vs `:492`) with slugs from the validated index; mutex discipline held. Nothing reopened. |
| 8 | **Double-"Projects" heading** | **LEAVE-AS-IS** | Real: `SidebarHeader.tsx:19` titles the workspace list "Projects" when `groupBy==='repo'`, directly below the new group's heading (`SidebarProjectsNav.tsx:43–45`). Architecture Risk 5 explicitly kept SidebarHeader untouched; the right fix (header unconditionally "Workspaces"?) changes pre-existing UX — a one-line **PM decision**, not a review-gate patch. qa.sh slice 1 will see it; ticket if it reads badly. |

---

## Must-fix (blocking)

**None.**

## Should-fix (non-blocking — follow-up ticket material)

1. **Stale `pageCache` across the ready→running (Regenerate) transition** — `WikiPage.tsx:177–184` clears the cache only on running→**ready**. On Regenerate, a slug shared with the old wiki takes the cache hit and renders the **old generation's** content behind the "Generating wiki…" banner until Ready clears it. The D-A6 invariant holds (partial mid-run content never survives as final) and it self-corrects at Ready, but it inverts the banner's promise. One-liner: also clear the cache when entering `running`.
2. **Pre-existing (001, not this branch):** the generate task removes `.status.json` on a parse-valid index (wiki.rs:480) without checking `all_pages_present`; a settled agent that listed a never-written page yields **Empty** (quiet) rather than **Failed** (loud). Worth a small 001-follow-up ticket.

## Leave-as-is nits (recorded, no action)

- Double-"Projects" heading (focus 8 ruling).
- `prettifySlug('api_reference') → 'Api Reference'` — cosmetic, replaced by real titles at Ready (D-A6 chose simple rendering).
- `SidebarProjectsNav` rebuilds `repoById` each render; heading is a plain `<div>` — trivial at tens of repos, consistent with siblings.
- `WikiEventFrame` declared structurally rather than importing `ServerEventFrame` — deliberate, commented.

## Release-gate reminders for the human

- **qa.sh / staging — six deferred live probes** (verification.md §F): progressive TOC growth, loud failure, socket-reconnect heal, two-hub scoping, the **amended** AC-4 network assertion (≤2 pinned-repo GETs on mount, zero for other repos, production build), and the **TCC quiet-open check — the spec's raison d'être**.
- **PR body must carry three statements:** (1) the AC-4 letter deviation note + rationale; (2) the D3 audit statement (text ready at tasks.md:82–83); (3) the StrictMode caveat (dev builds → up to 4 GETs; QA on a production build).
- The qa.sh wording amendment is already in spec.md — no further doc action needed.
