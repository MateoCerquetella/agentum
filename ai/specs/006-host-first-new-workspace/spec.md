# Spec: host-first New Workspace

## Goal

Let a user create a new workspace by first choosing a host, then a repo on that
host, so the New Workspace flow mirrors `agentum terminal` and can't target a
repo whose path isn't a git repository.

---

## User Value

A developer picks the host, then a repo on it (broken / non-git repos are
disabled), and creating a workspace on a remote project just works — instead of
the cryptic `fatal: not a git repository` 400 from selecting a misregistered repo.

---

## Requirements

- **Host selector** — the New Workspace composer (`NewWorkspaceComposerCard`)
  leads with a host picker: local + each configured SSH host (from the hosts
  slice / repo registry). Defaults to the active host (else local).
- **Host-scoped repo list** — the repo picker shows only the selected host's
  repos (local repos under "local"; a remote host's repos under that host).
- **Guard non-git repos** — a repo whose `detected` probe is non-authoritative
  (not a git repo on its host) is shown disabled with a "not a git repository on
  `<host>`" hint and cannot be selected.
- **Friendly create error** — `POST /api/worktrees/create` returns a
  human-readable error naming the repo, path, and host when the target isn't a
  git repository, instead of the raw `git` fatal.

---

## Acceptance Criteria

- [ ] New Workspace shows a host selector listing **local + each configured SSH
      host**; switching hosts re-scopes the repo picker to that host's repos.
- [ ] A repo whose path is not a git repo on its host appears **disabled** with a
      "not a git repository on `<host>`" hint and cannot be chosen.
- [ ] Creating a workspace on a valid repo under the selected host **succeeds**
      (worktree created on that host) — verified for one local and one SSH repo.
- [ ] A create against a non-git repo returns a **readable** error naming
      repo + path + host (no raw `fatal: not a git repository` reaches the user).
- [ ] Selecting a **disconnected** SSH host surfaces the existing Connect
      affordance before create is allowed.
- [ ] The flow requires **no** repo selection until a host is chosen (host → repo
      → name order), matching the TUI.

---

## Dependencies

- `002-sidebar-host-grouping` — host model + hosts slice (`hostMetaByKey`,
  `hostKeyForRepo`) reused for the selector and host→repo scoping.
- The remote git/worktree/agent layer (`host_runtime`, host-aware
  `worktrees::create`) and the `worktrees/detected` authoritative flag.

---

## Risks

- **Stateful composer** — `NewWorkspaceComposerCard` tracks
  `selectedRepoConnectionId` / `sshStatus` / `requiresConnection` /
  `connectInProgress`; the host selector must keep these consistent when the
  host (and thus the eligible repo set) changes.
- **Git-repo probe cost** — "is a git repo on host" relies on `detected`'s
  authoritative flag, which is one probe per repo (and an SSH round trip for
  remote hosts); may need caching / lazy evaluation to keep the dialog snappy.
- **Default host UX** — defaulting to the active host vs local; getting it wrong
  adds a click. (Confirm at PM.)

---

## Notes

**Root cause this addresses (observed):** `FinanzasArgy`'s registered remote path
`/home/malloc/Developer/projects/CerqueTech/FinanzasArgy` is not a git repo on
the remote, so `git worktree add` there 400s. The backend create is otherwise
healthy (confirmed: create succeeds on `agentum`, fails only on FinanzasArgy).

**Out of scope (future specs):**
- Opening an existing project *into its tmux* from the sidebar (spec 005's
  deferred sibling — part B).
- Creating a brand-new repo / cloning from the composer.
- Repos that exist on multiple hosts.

**Decisions:**
- Order = **host → repo → name/branch**, mirroring `agentum terminal`.
- Non-git repos are **disabled, not hidden** (so the user sees why).
- **Default host** = the host of the currently-active workspace/project; else
  **local**. (PM-resolved — removes the open UX risk.)
- The friendly create-error stays in this spec (small, and core to "fix the New
  Workspace flow"); not split out.
- "Is a git repo on host" reuses the existing `worktrees/detected` authoritative
  flag; cache the result per (host, repo) for the dialog's lifetime to avoid
  repeated SSH probes.
