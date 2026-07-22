# Handoff 03: Developer to Tester

**Spec:** 025 — Project-Scoped Integrations and Board Isolation

**From:** Developer

**To:** Tester

**Date:** 2026-07-22

**Verdict:** PASS WITH ENVIRONMENTAL RUST-GATE BLOCKER

## Delivered F1 → F4

1. Project-owned integration configuration now lives in `ProjectIntegrationsSection`, mounted only by non-folder `RepositoryPane`. It owns the provider, the existing host-aware GitHub Projects/status editor, and one exact Linear workspace/project binding. Global Integrations no longer renders a repository selector or project binding editor; account credentials and account/pipeline dictionaries remain per architecture.
2. `Repo.linearProjectBinding` uses the exact atomic shape from architecture. The repo sanitizer trims required strings, permits explicit `null`, rejects incomplete or non-HTTPS values, clones/freezes the result, and retains the existing per-repo serialized update chain. The server's flattened repo store has an object/null/unrelated-field round-trip regression.
3. `ProjectHubPage` no longer imports or mounts global `TaskPage` and no longer renders the editable tracker strip. It mounts `ProjectTasksPage`, which resolves a new generation-scoped immutable authority from the explicit repo. GitHub reads keep `repoId` on the host-aware binding seam; Linear reads verify the persisted workspace, exact project response, returned issue workspace/project/team identity, and never read global Linear selection/resume state.
4. Locked GitHub mode suppresses `ProjectPicker` and global board resolution, rejects a table with the wrong Projects ID, and permits mutations/dialog/workspace actions only for a row whose registered repo includes the active repo. Locked Linear state is cleared on each scope generation, late results are ignored, and reads/creates/status updates validate scope plus exact team. Workspace modal data now supports `requiredProjectTaskScope`, locks all sibling repos, and revalidates before submission, after trust/connection gates, and before `createWorktree`.
5. Existing Tauri Linear commands now implement exact project lookup, exact project issue list, issue lookup, team workflow states, create, and update through the existing GraphQL/runtime seam. Returned issue/project records are stamped with workspace/project identity used by the renderer guard.

## Primary files

- `ui/src/components/settings/ProjectIntegrationsSection.tsx`
- `ui/src/shared/linear-project-binding.ts`
- `ui/src/lib/project-task-scope.ts`
- `ui/src/lib/project-task-scope-guard.ts`
- `ui/src/components/project-hub/ProjectTasksPage.tsx`
- `ui/src/components/project-hub/LockedLinearProjectTasks.tsx`
- `ui/src/components/github-project/ProjectViewWrapper.tsx`
- `ui/src/components/new-workspace/create-workspace-wizard-model.ts`
- `ui/src/hooks/useComposerState.ts`
- `crates/agentum-desktop/src/commands/linear.rs`
- `crates/agentum-server/src/routes/repos.rs`

(`ui/` means `crates/agentum-desktop/ui/`.)

## Verification evidence

- Focused F1–F4 Vitest command: **PASS**, 14 files / 75 tests.
- Production UI build: **PASS**, 7,229 transformed modules in 1m18s; only existing dynamic-import/chunk-size warnings.
- `cargo test -p agentum-server --lib routes::repos::tests`: **PASS**, 20 passed / 768 filtered out.
- `cargo fmt --all`: applied; Rust source parses and is formatted.
- `git diff --check`: **PASS** during each slice and before final tests.
- Desktop Linear Rust test command: **BLOCKED BEFORE DESKTOP SOURCE COMPILATION**. `agentum-desktop`'s build script exits because `../../target/release/libsherpa-onnx-c-api.dylib` is absent. This is a workspace prerequisite unrelated to this diff; it also prevents `cargo test --workspace --lib`.
- A raw `tsc --noEmit` is not a supported clean gate in this worktree: it reports the repository's existing Electron-era relative shared imports and other baseline type errors. The production Vite build is green.

## Tester priorities

1. Use two Agentum repos on the same Linear account, bind different Linear project IDs, reload, and verify each Project Hub renders only its own exact project. Switch while throttling the first list call and confirm no stale row/dialog/error appears.
2. Bind two repos to different GitHub Projects boards and repeat the switch test. Confirm there is no picker, provider tab, tracker editor, or global active-project fallback in either hub.
3. Clear/revoke each binding and verify the project-named fail-closed state opens that repo's Project Integrations section, never global Integrations.
4. Exercise one valid and one deliberately mismatched create/status/workspace action for each provider. For GitHub shared boards, confirm cross-repo rows remain readable but their edit/start controls do not mutate through the active repo.
5. Cover one local GitHub repo and one SSH-backed Linear repo. Inspect traffic to verify GitHub binding requests carry `repoId`, and Linear project/list/create/update commands carry the bound workspace/project IDs.
6. Restore/build the Sherpa release dylib, then run `cargo test -p agentum-desktop --lib commands::linear::tests`, `cargo test --workspace --lib`, and the final `git diff --check`.

## Known non-product limitation

No architecture send-back was required. The sole incomplete gate is the missing local native Sherpa build artifact. Do not weaken scope validation or restore global fallbacks to work around unavailable external fixtures.
