# Handoff 02: Architect to Developer

**Spec:** 025 — Project-Scoped Integrations and Board Isolation

**From:** Architect

**To:** Developer

**Date:** 2026-07-22

**Verdict:** PASS

**Architecture:** `ai/specs/025-project-scoped-integrations-and-board-isolation/architecture.md`

## Implementation Contract

Implement in strict order F1 -> F2 -> F3 -> F4. Do not begin a later slice until the earlier slice's focused tests, UI build, and `git diff --check` pass. The architectural safety boundary is mandatory: `ProjectHubPage` stops mounting global `TaskPage` and instead mounts a new `ProjectTasksPage` that resolves an immutable repository task scope before dispatching a locked GitHub or Linear view.

## Binding Decisions

1. Add `Repo.linearProjectBinding` with persistence key exactly `linearProjectBinding` and shape `{ workspaceId, workspaceName, projectId, projectName, projectUrl? }`.
2. IDs are authority; names and URL are display metadata. `null` means explicit clear, absent means legacy/unconfigured, and malformed partial objects are rejected.
3. `auto` and absent provider are unbound inside Project Hub. Do not consult global defaults, selected workspaces, remembered projects, or same-name matches.
4. Preserve the existing GitHub server binding routes. They are keyed internally by normalized GitHub slug even though the UI is repository-owned.
5. Every bound scope carries `{scopeKey, generation, repoId}`. Compare all three against a live scope ref before accepting async results or starting actions.
6. The standalone Tasks route retains global explorer behavior. Locked behavior belongs only to Project Hub.

## Grounding Corrections the Implementation Must Include

- Linear project issue reads and mutations are currently stubbed in `crates/agentum-desktop/src/commands/linear.rs`. F3 implements exact project lookup/list reads; F4 implements issue lookup, team states, create, and update through those existing commands. Do not create a parallel client transport.
- Global Integrations loses repository provider and GitHub binding controls. Account-wide GitHub label dictionaries, Linear workflow-state dictionaries, credentials, and Harness toggles remain because the PM decision only assigns repository provider/board ownership. Escalate if product means “connections only” literally and wants those pipeline controls moved.
- Two Agentum repos resolving to the same GitHub slug share a binding under the existing server contract. Escalate before changing server storage if distinct per-registration bindings are required.

## Mandatory Slice Sequence

### F1 — Settings relocation

- Create `ProjectIntegrationsSection` under repository settings with provider selection and GitHub binding editor.
- Remove `GithubProjectsBoardEditor` and repository binding language from global Integrations.
- Add stable section targeting/search terms and F1 harness id `sdd-project-scope-f1-settings-relocation`.
- Run the exact F1 commands in the architecture.

### F2 — Linear binding persistence

- Add the shared immutable binding type, normalization helper, Repo update whitelist/sanitizer, serialized persistence tests, and settings workspace/project picker.
- Persist exact IDs; clear with `null`; never infer a project from its name.
- Add server flattened-field round-trip tests and F2 harness id `sdd-project-scope-f2-linear-persistence`.
- Run the exact F2 commands in the architecture.

### F3 — Locked read surfaces

- Add immutable scope variants and the single `scopeKey` helper.
- Replace embedded `TaskPage` with `ProjectTasksPage`.
- Add locked GitHub mode without `ProjectPicker` or global resolution.
- Add `LockedLinearProjectTasks` using explicit workspace/project IDs only.
- Implement exact Linear Rust project reads and require returned issue identity to match.
- Clear all provider UI state synchronously at each generation change.
- Add F3 harness id `sdd-project-scope-f3-locked-reads` and run the exact F3 commands.

### F4 — Guarded actions and workspace flows

- Add a central scope guard and apply it to all list/detail/pagination results, mutations, post-mutation refreshes, and workspace callbacks.
- Validate GitHub row repository ownership and Linear workspace/project/team identity in addition to freshness.
- Add `requiredProjectTaskScope` to the workspace wizard, lock the repo, and revalidate before creation and after connection gates return.
- Complete Linear Rust issue/team/mutation commands.
- Add F4 harness id `sdd-project-scope-f4-guarded-actions`; keep all four harness ids in order in the final block.
- Run the exact F4/final commands in the architecture.

## Acceptance Watchpoints

- An unbound or unavailable repository shows a scoped empty/error state with a link to that repository's Project Integrations section. It never opens global settings and never silently falls back.
- Switching repository, provider, or binding immediately clears selection, results, pagination, search, errors, and open modals before the next request resolves.
- Deferred-response tests must cover A -> B switches for GitHub -> Linear, Linear -> GitHub, and same-provider/different-binding pairs.
- Cross-repository rows may remain visible on a genuinely shared GitHub board, but repo-backed mutations and workspace actions are read-only unless the row resolves to the active scope repository.
- Linear creation always includes the bound project ID and a team from the exact project's team set.
- Local GitHub and SSH Linear fixtures must independently survive reload and prove exact external IDs in API/command traffic.
- Existing gated-run local-only policy remains unchanged.

## Risks and Send-Back Conditions

Send back to PM/Architect before implementation diverges if:

- all non-credential pipeline controls must also leave global Integrations but no destination is specified;
- GitHub binding must distinguish Agentum registrations with the same GitHub slug;
- the Linear API cannot provide enough project/workspace identity to validate issue reads;
- a proposed implementation requires global TaskPage state or fallbacks inside Project Hub.

## Developer Recommendation

Start with F1 only. It is the smallest visible ownership change and establishes the repository settings target used by all later unbound/unavailable states. Preserve unrelated worktree changes; this architecture handoff intentionally does not update `ai/STATE.md` or implementation files.
