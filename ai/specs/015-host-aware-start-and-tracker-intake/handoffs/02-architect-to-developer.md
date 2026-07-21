# Handoff 02 — Architect → Developer

- **Spec:** 015-host-aware-start-and-tracker-intake
- **Date:** 2026-07-13
- **From:** Architect (grounded on `origin/develop` @ `4f98453f`)
- **To:** Developer
- **Blueprint:** `ai/specs/015-host-aware-start-and-tracker-intake/architecture.md`
  (read it first — this is the compact index).

## Decisions resolved (binding)

1. **D6 key = (path, connection_id)**, exact string equality, `None == None`
   for local. NOT `host_id` (two desktop connections → one server host must
   stay two entries — the UI buckets by `connection_id`), NOT both (host-map
   drift between adds of one connection would break AC 2 idempotency).
   Corollary: `update()` adds `connectionId` to its immutable-keys skip list;
   `hostId` stays editable.
2. **F2 hop payload** (multi-match only; exactly-one keeps today's
   `launchWorkItemDirect` byte-equivalent):
   `openModal('new-workspace-composer', { linkedWorkItem (+ best-effort body),
   prefilledName, initialRepoId: <local match, else first>, telemetrySource:
   'sidebar' })`. Deliberately **no** `startGatedRun`, **no**
   `initialBaseBranch`. Detection = new pure
   `components/github-project/start-work-repo-match.ts::classifyStartWorkRepoMatches`.
   Callers changed: `ProjectViewWrapper.handleStartWork`, its
   `GitHubItemDialog.onUse` (share one `startWorkForItem`), and
   `handleOpenDialog`'s multi arm (repo-backed dialog with the seed).
   `github-item-checks-tab.tsx:189` / `pull-request-checks-tab.tsx:189` stay
   as-is (repo already determined by their surface — no slug matching there).
3. **F3 Linear seam = NO new server route.** Spec premise was stale: 013 F3
   is landed (`CreateWorkspaceWizard.tsx:1676-1830` files Linear via
   `runtime-linear-client.ts::linearCreateIssue` → native
   `api.linear.createIssue`). The Tracker panel reuses those exact client fns.
   `linear.rs`/`task_sink.rs` untouched. Consequently **F3 has zero Rust** and
   the spec's "Linear-create route test with stubbed sink" verify line is
   void.
4. **F3 component boundary:** `ProjectTrackerConfig` (ProjectHubPage) widens
   `{path}` → `{repo}` and mounts NEW sibling
   `components/project-hub/TrackerIntakePanel.tsx` (+ thin
   `use-tracker-intake.ts`); `ProjectBindingEditor` untouched except wiring
   its existing `onBound` to a refresh bump. Panel does its own
   `getProjectBinding` read (WorkItemsField precedent).
5. **F3 "Start gated run" = the spec-008 pre-armed composer hop**
   (`startGatedRun: true` + `linkedWorkItem` + `initialRepoId`). NEVER call
   `startGatedWork` directly from the panel — `start_work` wants a freshly
   created worktree (`harness.rs:460-462`); the wizard's `maybeStartGatedRun`
   (`useComposerState.ts:2291-2320`) is the one entry path and carries the
   `deriveIssueSideEffectGate` precondition set (GitHub-only per D3 falls out
   of the gate).
6. **State model is add-only** in `create-issue-intent-model.ts`:
   `TrackerIntakePhase` (adds `'filed'`), `FiledIssue`,
   `deriveTrackerIntakePhase` (precedence: filing > drafting > error > filed >
   review > idle), `deriveFiledGatedRunGate` (composes
   `deriveIssueSideEffectGate`). Do not edit 013's existing exports.

## Build order (tests FIRST in every step)

1. **F1a (Rust):** `repos.rs` — pure `register_repo(&mut Vec<Repo>, …) ->
   (Repo, bool)` extracted from `append_repo`; 6 unit tests from
   architecture §2.2 (no fs, no env mutation); `update` skip-list +
   `scope_repo_pairs` locals-first partition (+ tests).
2. **F1b (UI):** `lib/find-repo-by-path.ts` `findRepoByPathPreferLocal` +
   test; swap the 8 path-fallback sites in `store/slices/hosted-review.ts`
   (:177/:191/:202/:215/:231) and `store/slices/github.ts` (:107/:1986/:2149).
3. **F2:** `start-work-repo-match.ts` + test; wire `ProjectViewWrapper`
   (shared `startWorkForItem`; revise the stale ":531 Project mode does not
   own the composer modal" comment — the modal is app-mounted, `App.tsx:1801`);
   re-check `project-dialog-state.ts` for the `length === 1` assumption.
4. **F3:** model fns + tests in `create-issue-intent-model.(test.)ts`;
   `use-tracker-intake.ts`; `TrackerIntakePanel.tsx`; `ProjectTrackerConfig`
   prop widening + `onBound` wiring.

**Do not ship F1 without F2**: post-F1, a two-host repo turns the board's
Start-work into a false "isn't added to Agentum" dialog until F2 lands
(architecture §3.1).

## Gates (per increment)

- **F1:** `cargo test -p agentum-server --lib` · `cargo fmt` ·
  `cargo clippy -p agentum-server` ·
  `bunx vitest run src/lib/find-repo-by-path.test.ts` ·
  `bun run build` (run vitest/build from `crates/agentum-desktop/ui`, bun not
  npm).
- **F2:** `bunx vitest run
  src/components/github-project/start-work-repo-match.test.ts
  src/components/github-project/project-dialog-state.test.ts` ·
  `bun run build`.
- **F3:** `bunx vitest run
  src/components/new-workspace/create-issue-intent-model.test.ts` ·
  `bun run build`. No Rust gates.
- Never gate on full vitest (~139 pre-broken fails) or bare `tsc` (~1650
  pre-broken errors).

## Line-drift warnings (012/013 in-flight on the same surface — re-ground before editing)

- `useComposerState.ts`: `submit` at `:2349` (createWorktree call `:2471`),
  `submitQuick` at `:2602` (call `:2681` — spec cited the call at :2602);
  `handleCreateIssueSubmit` `:1519` (spec said :1510),
  `handleGenerateIssueBody` `:1615` (spec said :1604);
  keep-selection effect `:1012-1020` ✔ as specced.
- `ProjectViewWrapper.tsx`: `handleStartWork` `:503-543` (spec said :503-526);
  second launch site (dialog onUse) `:805`.
- `TaskPage.tsx`: `openComposerForItem` `:2346-2385` (spec said :2345-2374).
- `repos.rs` `:127-161/:134/:215/:358-378` ✔; `harness.rs` `start_work`
  handler `:516` (StartWorkRequest `:460`); `harness-client.ts`
  `startGatedWork` `:171` ✔; `github.rs` create `:219`/draft `:302-ish` ✔
  region; `worktree-list-groups.ts` `hostKeyForRepo` `:246-248` (spec said
  :242-247).
- Invariants: no serde aliases, no new spawn/create paths, streaming
  untouched, `unwrap_or(LOCAL_HOST_ID)` stays, collapsed legacy entries are
  never auto-rewritten (D4).
