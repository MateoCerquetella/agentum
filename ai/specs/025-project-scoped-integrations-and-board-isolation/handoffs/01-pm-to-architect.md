# Handoff 01 — PM → Architect

- **Spec:** `025-project-scoped-integrations-and-board-isolation`
- **Date:** 2026-07-22
- **From:** PM (autonomous SDD loop; the explicit `validate_handoff` PM gate was
  used because `ai/roles/pm.md` is absent)
- **To:** Architect
- **Artifact:** `ai/specs/025-project-scoped-integrations-and-board-isolation/spec.md`

## Verdict

PM gate: **PASS** after one refinement pass.

| Gate item | Verdict |
| --- | --- |
| One slice | PASS — one authority boundary: a project's persisted integration binding is the only external board its embedded Tasks surface can display or operate. Configuration ownership, locked rendering, and guarded actions are the end-to-end parts of that boundary, not independent products. |
| Problem before solution | PASS — the Problem opens with fragmented ownership and the user-visible risk of viewing, filing, or starting work from another project's board. |
| Persona and value | PASS — Mateo, a multi-project operator, feels the failure while configuring or switching client projects; local and SSH operators are covered explicitly. |
| Acceptance criteria | PASS — ten numbered criteria use observable render, persist, remove, resolve, reject, clear, ignore, preserve, and pass outcomes. Provider-paired browser QA is explicit. |
| Scope / non-goals | PASS — credentials and authentication redesign, global Tasks changes, multi-board projects, new providers, and Harness/launch behavior are excluded. |
| Grounded in code | PASS — `RepositoryPane`, `GithubProjectsBoardEditor`, `ProjectBindingEditor`, `getProjectBinding`, `resolveBoardProject`, keyed Project Hub mounts, `TaskPage`, Linear project reads, the repo update queue, and global connection management were verified in this worktree. |
| Invariants | PASS — repo identity is authoritative; incomplete or stale scope fails closed; account credentials remain global; sibling repo data, SSH host routing, one launch path, YOLO translation, push streaming, and session UUID behavior remain intact. |
| Harness wiring | PASS — four ordered `feature_list.json` entries cover the single boundary, with focused unit/component/build/Rust assertions in `verify.sh` and provider-paired isolation/race/mutation assertions in `qa.sh`; both gates must be green. |
| Human question | PASS — none blocks architecture. Storage shape and component factoring are explicitly delegated within locked observable constraints. |

## PM decisions locked

1. Treat this as one vertical isolation slice: Project Settings establishes one
   repo-owned binding, and embedded Tasks consumes only that binding for reads
   and actions.
2. Account authentication remains in global Integrations. Provider choice,
   GitHub board/status mapping, and Linear workspace/project binding belong only
   to Project Settings for the current `repo.id`.
3. Embedded Tasks has no escape hatch through source tabs, repo selectors,
   Linear Projects/Views, workspace-wide lists, cached global collections,
   resume state, or `activeProject`. The standalone Tasks explorer keeps those
   cross-project capabilities.
4. Missing, malformed, loading, inaccessible, mismatched, or stale scope fails
   closed with a project-named empty state and an Open Project Settings action.
5. Repo switches and every read or mutation validate the captured scope against
   the live repo plus bound external-project identity. A late or hostile payload
   cannot render or mutate.
6. Isolation and binding persistence must behave identically for local and SSH
   repos; GitHub binding lookup continues passing `repoId` through the existing
   host-aware seam.

## Code evidence verified for architecture

- `RepositoryPane.tsx` currently owns the per-repo `trackerProvider` control,
  while `IntegrationsPane.tsx::GithubProjectsBoardEditor` and
  `ProjectHubPage.tsx::ProjectTrackerConfig` duplicate project-owned GitHub
  binding editing elsewhere.
- `Repo` exposes `trackerProvider`; `repos.ts::RepoUpdate` is a whitelist and
  `updateRepo` serializes writes per repo. The server repo registry uses
  `#[serde(flatten)] extra`, so architecture must add a typed/sanitized client
  field without losing unknown fields or sibling records.
- `ProjectHubPage` keys embedded `TaskPage` by `repo.id` and calls
  `getProjectBinding({ workdir, repoId })`, but `TaskPage` still derives broad
  repo selection and selected Linear workspace from global state and renders
  provider/source and Linear scope controls in embedded mode.
- `resolveBoardProject` already prevents an unbound embedded GitHub repo from
  borrowing global `activeProject`; its explicit pending/binding/none behavior
  is the fail-closed precedent, while its per-repo pick tier is not authority
  for the new locked surface.
- The Linear store already provides `listLinearProjectIssues(projectId,
  workspaceId, ...)`, so locked Linear reads can use the narrow endpoint rather
  than workspace-wide Projects, Views, or issue collections.

## Architect output expected

Create `architecture.md` that pins:

- the typed, atomic Linear binding shape and persistence/sanitization path;
- one immutable `ProjectTaskScope` identity and its loading/bound/unbound/
  unavailable transitions for both providers;
- the component boundary that keeps standalone Tasks behavior intact while
  making embedded Tasks structurally incapable of cross-project navigation;
- generation/cancellation and response-application guards for A→B switches;
- validation at every embedded read and mutation seam, including gated-run
  intake and workspace start;
- exact tests and `feature_list.json` ordering for F1→F4, plus concrete
  `verify.sh` and `qa.sh` commands/assertions.

Then write the Architect→Developer handoff. No human product decision is needed
before architecture begins.
