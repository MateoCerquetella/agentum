# Spec 025 implementation tasks

## F1 — Project Integrations ownership

- [x] Add one stable Project Integrations section to non-folder repository settings.
- [x] Move provider selection and the existing GitHub Projects/status editor into that section.
- [x] Remove repository selection and GitHub board binding from global Integrations.
- [x] Remove the Project Hub tracker configuration strip/editing location.
- [x] Add settings ownership/search regression coverage.

## F2 — Atomic Linear project binding

- [x] Add `LinearProjectBinding` and `Repo.linearProjectBinding` with the architecture's exact wire shape.
- [x] Normalize, validate, clone, freeze, explicitly clear with `null`, and retain serialized per-repo writes.
- [x] Add exact workspace/project selection in Project Settings without name inference.
- [x] Cover rapid writes, malformed values, null clears, and sibling preservation.
- [x] Cover flattened server object/null round trips and unrelated-field preservation.

## F3 — Locked Project Tasks reads

- [x] Add immutable `ProjectTaskScope` states and one non-lossy `scopeKey` helper.
- [x] Replace embedded global `TaskPage` with `ProjectTasksPage`.
- [x] Add GitHub locked mode that bypasses global resolution and hides the project picker.
- [x] Add `LockedLinearProjectTasks` using only bound workspace/project IDs.
- [x] Implement local Linear exact project and project-issue commands with response identity.
- [x] Fail closed for unbound, malformed, disconnected, inaccessible, or stale scopes.
- [x] Preserve explicit `repoId` in host-aware GitHub binding reads and exact IDs for local/SSH Linear reads.

## F4 — Stale/write/modal enforcement

- [x] Add central `{scopeKey,generation,repoId}` capture/current checks.
- [x] Clear locked provider state and ignore late list/detail/status results on scope changes.
- [x] Guard Linear create/update/refresh by workspace, project, and team identity.
- [x] Guard locked GitHub rows, mutations, dialogs, and workspace starts by board ID and active repository ownership.
- [x] Add `requiredProjectTaskScope` to the workspace wizard, lock repository selection, and revalidate before and after gates and before creation.
- [x] Implement local Linear issue lookup, team-state lookup, create, and update commands.
- [x] Add focused scope, control-removal, persistence, race/action, modal, runtime, and Rust coverage.

## Verification

- [x] Focused UI tests pass (14 files, 75 tests).
- [x] Production UI build passes (`vite build`, 7,229 modules, final run 1m18s; only existing chunk/dynamic-import warnings).
- [x] Server repo route tests pass (20 passed, 768 filtered out).
- [ ] Desktop Linear Rust tests are blocked before compilation by the missing pre-existing `libsherpa-onnx-c-api.dylib` build prerequisite.
- [ ] `cargo test --workspace --lib` is blocked by the same desktop build prerequisite.
- [x] `git diff --check` passes.
