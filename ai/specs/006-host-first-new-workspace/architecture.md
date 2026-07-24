# Architecture Notes — host-first New Workspace

## Components

Files to touch (named; nothing else changes):

1. **`ui/src/hooks/useComposerState.ts`** — the state owner. Today it derives
   `eligibleRepos` (repos with a path), picks a default `repoId`, and exposes
   `selectedRepo*` (isGit / connectionId / sshStatus / requiresConnection). Add:
   - `selectedHostKey` state + setter.
   - `eligibleHosts` — distinct hosts derived from `eligibleRepos` via
     `hostKeyForRepo(repo)` (reused from spec 002, `worktree-list-groups.ts`),
     joined with `hostMetaByKey` for labels.
   - `hostScopedRepos` — `eligibleRepos` filtered to `selectedHostKey`.
   - Default `selectedHostKey` = active workspace's host, else `local`
     (PM-resolved); default `repoId` recomputed from `hostScopedRepos`, and
     **reset** when `selectedHostKey` changes.
   - `isGitOnHost(repoId)` — backed by a per-`(hostKey, repoId)` cache of the
     `worktrees/detected` `authoritative` flag (see Data Flow).

2. **`ui/src/components/NewWorkspaceComposerCard.tsx`** — add a **Host selector**
   row above the existing `RepoCombobox`; pass `hostScopedRepos` (not all
   eligible) + a `disabledRepoIds`/reason map to the combobox. No other layout
   change. `NewWorkspaceComposerModal.tsx` just threads the new props.

3. **`ui/src/components/repo/RepoCombobox.tsx`** — accept an optional
   `disabledRepoIds: Map<string,string>` (id → reason); render those rows
   disabled with the reason as a hint. Already shows `connectionId`; no grouping
   needed since the list is pre-scoped to one host.

4. **`crates/agentum-server/src/routes/worktrees.rs`** (`create`) — when the
   failed `git` output stderr matches "not a git repository", return
   `ApiError::BadRequest("<repo path> on <host> is not a git repository — re-add
   the project with the correct path")` instead of the raw fatal.

Untouched: sidebar, the create success path, the hosts slice itself, all other
composer consumers (TaskPage, WorktreeJumpPalette — they pass `repoIdOverride`
and bypass host selection).

---

## APIs

- No new endpoints. Reuse `GET /api/worktrees/detected?repoId=` — its
  `authoritative` flag (`source:"git"` vs `"metadata-fallback"`) is the
  "is a git repo on this host" signal (this is exactly what flagged FinanzasArgy).
- `POST /api/worktrees/create` — unchanged contract; only the error *message*
  on the non-git failure path becomes human-readable.

---

## Data Flow

1. Dialog opens → `useComposerState` computes `eligibleHosts`, defaults
   `selectedHostKey`, and `hostScopedRepos`.
2. For the scoped repos, lazily call `worktrees/detected` (cache per
   `(hostKey, repoId)`); `authoritative === false` → repo is disabled with
   "not a git repository on `<host>`".
3. User picks host → repo list re-scopes + `repoId` resets → name/branch → create.
4. Create still routes through `worktrees::create` → `load_host_for_repo` →
   `git_in_dir` on the repo's host (already host-aware). The guard makes a
   non-git create unreachable from the UI; the friendly error is the backstop.

---

## Important Decisions

- **Guard on `detected.authoritative`, not `repo.kind=='git'`.** The registry
  marks FinanzasArgy `kind:git` yet its remote path isn't a repo — the registry
  flag is stale/optimistic, so correctness requires the live probe.
- **Filter in `useComposerState`, not the card.** State ownership already lives
  in the hook; keep the card presentational. Why: one source of truth, no prop
  round-trips.
- **Cache the probe per `(host, repo)` for the dialog lifetime.** Avoids an SSH
  round trip on every keystroke/re-render. Why: snappy dialog over freshness
  (repos rarely change git-ness mid-dialog).
- **Friendly error by stderr match, no new error type.** Minimal; reuses
  `ApiError::BadRequest`.

---

## Risks

- **Probe cost / latency (SSH per repo).** → Mitigation: cache per
  `(host,repo)`; probe only the selected host's repos, lazily; show repos
  enabled-pending until the probe resolves rather than blocking the dialog.
- **State desync on host switch** (stale `repoId` from another host). →
  Mitigation: recompute/reset `repoId` from `hostScopedRepos` whenever
  `selectedHostKey` changes; covered by a `useComposerState` unit test.
- **Other composer entry points** (TaskPage/JumpPalette use `repoIdOverride`). →
  Mitigation: when `repoIdOverride` is set, skip the host selector entirely
  (host inferred from the overridden repo) — no behavior change for them.
- **Default-host correctness.** → Accepted: PM-resolved (active host else local).
