# Spec 026 — Architecture

- **Phase:** Architect
- **Date:** 2026-07-21
- **Verdict:** PASS — proceed in two ordered increments.

## Current-state findings

The selected repository already reaches the binding API as `{workdir, repoId}`.
`github_projects::get_binding` resolves the real origin through
`util::resolve_tracker_slug`, so SSH repositories execute against their registered
host. When `repoId` is present, the route delegates to
`project_trackers::compatibility_get`; the current worktree also contains the
Spec 025 canonical row and CAS store needed to distinguish `migrated` from
`configured` data.

The wizard has two remaining fidelity gaps:

1. `getProjectBinding` returns `{slug, binding}`, but `TrackerSection` discards
   `slug`. Consequently `deriveIssueOptions` accepts open issues from every
   repository in the configured GitHub Project.
2. Binding requests are guarded by `Repo.id + workdir`, while table requests are
   guarded only by GitHub Project identity. If projects A and B use the same
   Project, a late A table response has the same `projectKey` as B and can be
   accepted after the switch.

The selected work item itself is already cleared synchronously by
`useComposerState.handleRepoChange`, and `applyLinkedWorkItem` plus
`deriveTrackerBindCoords` remains the one persisted-ticket path.

## Decisions

### D1 — The binding response carries the authoritative repository scope

Keep `GET /api/github/project-binding` as the only wizard read. For calls with
`repoId`, the server:

1. resolves that registered repo's origin with `resolve_tracker_slug`;
2. reads only the canonical row owned by that same `Repo.id`;
3. compares the canonical `github.repository_slug` with the resolved slug using
   trimmed, ASCII-case-insensitive equality; and
4. returns `{slug, binding}` only after that comparison passes.

A mismatched `migrated` row is deleted with its current revision and migrated
again from the exact resolved slug. A concurrent write produces the existing
revision conflict rather than being overwritten. A mismatched `configured` row
is preserved and returns `409` with error code `tracker_target_mismatch`. No
slug-keyed or globally active binding is consulted after `repoId` is present.

The existing partial implementation in
`routes/project_trackers.rs::compatibility_get` is the reuse seam; Developer
must complete and test it rather than introduce a wizard-only endpoint.

### D2 — Binding resolution is a repo-and-slug identity, including typed failure

Extend `PickerBindingResolution` so a resolved value contains both the binding
and the server-returned `repositorySlug`. Its failed variant carries the
classified error code. `TrackerSection` maps
`GithubProjectsBindingError.code === "tracker_target_mismatch"` to an explicit
reconfigure state; other host/auth failures remain fail-closed and retryable.

For a selected git repo, `loading`, `absent`, and `failed` all resolve to no
Project and zero rows. `settings.githubProjects.activeProject` remains eligible
only when there is no selected git repo, preserving non-project legacy callers
without weakening this wizard's closed scope.

The Configure/Change control is repo-scoped for both local and SSH git repos.
`ProjectBindingEditor` receives the same `workdir + repoId`; the server owns host
selection and slug validation. An unreachable or missing SSH host therefore
fails through the existing typed route error and never retries locally.

### D3 — Repository slug is a mandatory row eligibility key

Add an optional `repositorySlug` argument to `deriveIssueOptions` and
`deriveTrackerIssueViewModel`. When supplied, an otherwise pickable row is
eligible only if `row.content.repository` equals that slug after trim and
ASCII-case normalization. A missing repository field is ineligible in a
repo-scoped picker. PRs, drafts, redacted rows, closed issues, malformed rows,
and duplicate URLs keep their existing exclusion rules.

The optional argument preserves the shared/global behavior for callers that do
not represent a selected repository. `TrackerSection` always supplies the slug
from its current resolved binding, never a client-derived remote or display
name.

### D4 — One scope key guards binding, cache projection, and table responses

Define a picker scope key from:

```text
bindingTargetKey(Repo.id, workdir)
  + normalized repositorySlug
  + pickerProjectKey(owner, ownerType, projectNumber)
```

On a repository change, the render-time binding target mismatch immediately
makes `resolved`, the table projection, status, count, and rows ineligible.
Table state is stored under the full scope key, not only `projectKey`. Every
cached-table projection is filtered with the current repository slug, and every
fetch/refresh completion compares its captured full scope key with the latest
one. This rejects A after switching to B even when A and B share one GitHub
Project.

Effects may clear old table state as cleanup, but correctness must come from
keyed eligibility during render; React effect timing is not a security boundary.

### D5 — Exact ticket persistence remains unchanged and is asserted

Repository switches continue to clear `linkedWorkItem` synchronously in
`useComposerState.handleRepoChange`. A visible option is transformed only by
`buildBindPayload`/`applyLinkedWorkItem`, and worktree creation persists
`trackerProvider: "github"` with that option's canonical issue URL. Creating
without a selection sends neither coordinate.

No changes are made to worktree creation, session launch, agent selection,
harness execution, or lifecycle transitions.

## Data and control flow

```text
selected Repo.id + registered workdir
              |
              v
 GET /api/github/project-binding
              |
 resolve origin on registered local/SSH host
              |
 canonical row by Repo.id -- mismatch --> repair migrated / reject configured
              |
       {resolved slug, binding}
              |
 full picker scope key (repo + slug + Project)
              |
 Project table/cache -- filter row.repository == resolved slug
              |
 visible issue -- existing applyLinkedWorkItem --> exact worktree URL
```

## Error and race handling

- Unknown repo, missing host, unreachable host, or origin-resolution failure:
  typed binding failure, zero rows, no connected badge, no local/global retry.
- Configured target mismatch: preserve the row, return
  `tracker_target_mismatch`, render a reconfigure affordance, and expose no
  table.
- Migrated target mismatch: CAS delete, exact-origin migration, then project the
  repaired row; a CAS collision returns conflict and exposes no stale binding.
- Absent binding: Configure tracker, zero rows, optional workspace creation.
- Repo or binding switch: old binding/table values are render-ineligible
  synchronously; all late completions compare the full scope key.
- Same Project across repositories: the table may be shared in cache, but its
  row projection is always slug-filtered and request acceptance remains
  repo-scoped.

## Exact edit map

### Server

- `crates/agentum-server/src/routes/project_trackers.rs` — finish the canonical
  slug comparison, migrated CAS repair, configured mismatch envelope, and
  two-repo/host tests in the existing compatibility adapter.
- `crates/agentum-server/src/routes/github_projects.rs` — keep resolution before
  compatibility projection; add route tests proving `repoId` never reaches the
  legacy/global binding path and returns the resolved slug.
- `crates/agentum-server/src/routes/util.rs` — reuse host-aware resolution; only
  add focused regression coverage if a missing-host/local-fallback case is not
  already pinned.

### Desktop UI

- `crates/agentum-desktop/ui/src/runtime/github-projects-client.ts` — preserve
  classified mismatch errors and the response slug in the typed result.
- `crates/agentum-desktop/ui/src/components/new-workspace/work-item-picker-model.ts`
  — resolved slug, typed failure, full scope key, and slug-aware row filtering.
- `crates/agentum-desktop/ui/src/components/new-workspace/work-item-picker-model.test.ts`
  — mixed-repository, absent repository, case normalization, and same-Project
  scope tests.
- `crates/agentum-desktop/ui/src/components/new-workspace/CreateWorkspaceWizard.tsx`
  — use the response slug, full-scope request guards, honest mismatch copy, and
  repo-scoped Configure/Change control for local and SSH.
- `crates/agentum-desktop/ui/src/components/new-workspace/create-workspace-wizard-model.ts`
  and its test — status derivation for mismatch/unconfigured states where pure
  model coverage is appropriate.
- `crates/agentum-desktop/ui/src/hooks/useComposerState.ts` tests — pin the
  existing repo-switch linked-item reset; do not duplicate the linkage path.

## Acceptance-criteria traceability

| AC | Implementation seam | Verification |
|---|---|---|
| 1 | D1 canonical `compatibility_get` | Rust matching/mismatch route tests |
| 2 | D2 closed-scope resolver/status | Vitest unbound selected repo with global active Project |
| 3 | D1/D2 provenance branch + typed error | Rust CAS repair/preservation tests; UI mismatch-state test |
| 4 | D4 full scope key | Vitest A/B same-Project deferred-response test |
| 5 | D3 slug-aware issue derivation | Vitest mixed repo/PR/draft/closed/redacted matrix |
| 6 | D1/D2 editor with `repoId` | Rust two-row byte-preservation test; editor request test |
| 7 | D1/D2 host-aware route | Rust SSH success/missing-host tests; UI fail-closed state |
| 8 | D5 existing bind/create seam | Composer repo-switch and linked/unlinked create tests |

## Build order and gates

1. **Binding identity fidelity:** complete server comparison/repair/error behavior
   and run focused `agentum-server` tests plus `git diff --check`.
2. **Wizard closed tracker scope:** thread slug and scope identity through the
   pure model and component, add row/race/linkage regressions, run focused
   Vitest, the Vite production build, relevant Rust library tests, and
   `git diff --check`.

Real desktop QA follows the spec's `qa.sh` matrix and must not be claimed from
unit tests: Agentum → unbound xcode-theme, mixed-repository Project, repeated
switches, SSH bound/unbound, then linked and unlinked workspace persistence.

## Invariant check

- The selected `Repo.id` and server-resolved origin are the only authority for
  a selected git project.
- No global tracker, sole binding, display name, or local-path retry is added.
- Explicit configuration is never overwritten automatically.
- Existing Project fetch/cache clients and exact-ticket creation seam are
  reused.
- Session spawn, YOLO translation, per-session UUIDs, push streaming, and
  harness gates are untouched.
