# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 001-autowiki
- **phase:** tester      <!-- idle | spec | pm | architect | developer | tester | reviewer -->
- **mode:** HITL         <!-- HITL (human in the loop) | auto -->
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

- **001-autowiki** — all 3 slices BUILD-GREEN (wiki-contract ✓, wiki-routes ✓ 14
  tests, wiki-view ✓ Vite build). `wiki-view` = **ready_to_test**: browser QA
  (`qa.sh`) pending — needs the running app. Not yet committed / PR'd.

## Decision log

<!-- append one line per decision, newest last: `YYYY-MM-DD — <decision>` -->
- 2026-06-30 — Adopted the `ai/` SDD scaffold; retired `docs/superpowers/`.
  Execution runs through the Harness Engine (`.harness/`), gated by
  `verify.sh` + `qa.sh`.
- 2026-06-30 — From the "Ara" idea, scoped spec **001-autowiki** as the first
  slice (AutoWiki: generate + browse a repo wiki on demand). Deferred graph
  memory + autonomous PR-review bot; kept `examples/harness-demo` (live-test
  fixture).
- 2026-06-30 — Found `new-idea` was 237 commits behind `origin/develop` (HEAD
  v0.26). Re-based onto a fresh `feat/autowiki` worktree off `origin/develop` @
  `fe1a2a6a`; re-grounded reuse-vs-build on current code. Mermaid diagrams pulled
  into v1 (renderer already draws them); persistence is on-disk `.agentum/wiki/`.
  PM gate passed → phase `pm` (ready for architect).
- 2026-06-30 — Architect blueprint complete (`architecture.md`): job-model generate
  (returns `sessionId`); on-disk `.agentum/wiki/` + `.status.json`; reuse
  `MarkdownPreview` as-is; git-ignore via `.agentum/.gitignore`; widen
  `wait_for_settle`/`teardown_session`/`gather_repo_context` to `pub(crate)`. Phase
  → developer; scaffolded `.harness/`, building slices behind the verify gate.
- 2026-06-30 — Slice 1 (wiki-contract) GREEN: `crates/agentum-server/src/wiki.rs`
  (WikiIndex/WikiPageMeta, parse_wiki_index, is_valid_slug, build_wiki_prompt) +
  `lib.rs` mod; 9 unit tests pass, fmt-clean (reverted unrelated mcp.rs fmt drift).
  Gate run scoped to `wiki::` (full lib suite times out locally). Next: wiki-routes.
- 2026-06-30 — Slice 2 (wiki-routes) GREEN: `routes/wiki.rs` (GET list/page + POST
  generate via `spawn_agent_into_pane` + the QA-capture recipe; on-disk
  `.status.json`; slug-traversal guard; job-model returns sessionId). Widened
  `wait_for_settle`/`teardown_session`/`gather_repo_context` to pub(crate) +
  re-exported from `harness`. 14 wiki tests pass. Next: wiki-view (desktop UI).
- 2026-06-30 — Slice 3 (wiki-view) BUILD-GREEN: `runtime/wiki-client.ts` +
  `components/wiki/WikiPage.tsx` (reuses `MarkdownPreview` as-is; empty/running/
  failed/ready states; workdir via `splitWorktreeIdForFilesystem`; `[[Title]]` nav;
  3s poll for running→ready) + 6 store/nav edits (`BookText` rail). `bun install` +
  `npm run build` ✓ (9m23s). All 3 slices green at the unit/build gate; browser QA
  + commit/PR remain.
