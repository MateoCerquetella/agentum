# Remote (SSH) projects: host-aware git/worktree ops + remote agent detection

**Status:** DONE (2026-06-05). All three work items implemented, `cargo build`
+ `cargo test --workspace --lib` (379 pass, +3 new host_runtime tests) +
`vite build` + relevant vitest suites all green. NOT yet validated against a
live remote host (the plan's "only true validation" — needs the Omarchy box).
Finishes the remote-project work begun in `2026-06-04-desktop-ssh-tmux-sessions.md`
(add/connect/browse + sessions-in-remote-tmux already shipped on staging ≤ 210179a).

## What shipped (file-by-file)
- **`host_runtime.rs`** (foundation): `HostCommandOutput {success, code, stdout,
  stderr}`; `git_in_dir(host, cwd, args)` (Local `git -C`, SSH `sh -c 'cd && git'`,
  GIT_TIMEOUT 120s); `is_git_repo`, `mkdir_p`, `read_file_bytes`, `path_exists`
  — all Local/SSH. Local-backend unit tests added.
- **`routes/repos.rs`** (item 1): `Repo.host_id`/`AddBody.host_id`; `append_repo`
  stores it; `resolve_repo_host_id` + `load_host_for_repo` (mirrors
  `load_host_for_session`); base-ref reads (`git_out`/`collect_refs`/the 3 handlers)
  now host-aware.
- **`routes/worktrees.rs`** (item 2): `create` (remote `mkdir -p` + `git worktree
  add` over SSH — the `os error 45` 500), `remove`, `detected`/`scan_git_worktrees`,
  `force_delete_branch` all host-aware; worktree paths built as POSIX strings under
  `<repo>/.claude/worktrees/<name>`.
- **`routes/git.rs`** (item 2 + cascade): `run_git`/`run_git_bytes`/`git_ref_exists`
  host-aware; `host_and_cwd_for` resolves a session's host; EVERY handler (status,
  status-entries, diff incl. `--no-index`, file incl. worktree-disk read, stage,
  commit, blob, conflict's rebase-dir check, branches/log/history/upstream/…)
  routes through the host.
- **TS client** (items 1 + 3): `reposAddRemote(connectionId, remotePath, hostId?)`;
  `AddRepoSteps` + `tauri/repos.ts addRemote` resolve `connectionId → hostId` via
  `resolveServerHostIdForConnection` and persist it; `Repo.hostId` type added.
  `detectRemoteAgentsViaServer` (new, in server-host-client) reads
  `/api/hosts/{id}/readiness`; `preflight.detectRemoteAgents` wired to it (was a
  `[]` stub) so the composer's remote Agent picker lists installed remote CLIs.

## Original plan below


## Problem
Remote SSH projects can be added, connected, browsed, and their agent **sessions
run in tmux on the remote host**. But everything else still runs **locally**, so
on a remote repo:
- `POST /api/worktrees/create` → 500 `os error 45` (ENOTSUP): the route does
  `std::fs::create_dir_all` + local `git worktree add` on a path that only
  exists on the remote.
- Create-Worktree **Agent picker** shows only "Blank Terminal": agent detection
  probes the LOCAL machine, not the remote host.
- (Latent) git status / branches / base-refs / worktree list+remove would all
  misbehave on a remote repo for the same reason.

## The mapping decision (the crux) — RECOMMENDED: store host_id on the repo
The repo currently stores `connectionId` = the desktop's **native SSH-target
id**. Server git/worktree/agent ops need a **server host id** (`/api/hosts`) to
use `host_runtime`. The server can't map native-target-id → host-id.

**Sessions already solved this**: a `Session` stores `host_id` (the server host
id), resolved CLIENT-side from `connectionId` at create time
(`resolveServerHostIdForConnection`), and the server resolves it with
`load_host_for_session` → `host_runtime`. **Mirror that for repos.**

- Add `host_id: Option<Uuid>` to the repo registry (`routes/repos.rs` `Repo` +
  `AddBody`). Keep `connection_id` for the desktop's native-SSH UI link.
- On remote add (`reposAddRemote`), the desktop resolves `connectionId → host_id`
  (already does this for sessions) and sends BOTH.
- Server: add `load_host_for_repo(repo) -> Host` mirroring `load_host_for_session`
  (repo.host_id ?? LOCAL_HOST_ID → store.get_host).

Rejected alternatives: (b) server reads the desktop's `~/.agentum/ssh-targets.json`
to map native ids — couples server to a desktop file, violates "/api/hosts is the
source of truth"; (c) pass host_id on every worktree op — repeats the resolution
everywhere and forgets-prone. (a) is consistent with sessions and central.

## Work items

### 1. Repo carries host_id (foundation)
- `routes/repos.rs`: `Repo.host_id` + `AddBody.host_id`; `append_repo` stores it.
- `runtime/server-repo-client.ts::reposAddRemote(connectionId, hostId, remotePath)`
  — resolve hostId in `AddRepoSteps.handleAddRemoteRepo` and pass it.
- `load_host_for_repo` helper in the server (shared by worktrees + git routes).

### 2. Host-aware git/worktree ops (the 500)
Add git-over-host helpers to `host_runtime` (it already has `ssh_stdout`):
`git_in_dir(host, cwd, args) -> stdout` = Local: `Command::new("git").current_dir`;
Ssh: `ssh host 'cd <cwd> && git <args>'` (shlex-quoted). Then route through it:
- `routes/worktrees.rs`: `create` (`git worktree add`, mkdir → remote mkdir via
  `ssh host mkdir -p`), `remove` (`git worktree remove`), `list`/HEAD/branch
  reads. Resolve the repo's host; Local path unchanged.
- `routes/git.rs`: status / branches / base-refs / upstream / conflict — same
  `git_in_dir` treatment when the session/repo is remote.
- Worktree paths for remote repos live under the remote
  `<repo>/.claude/worktrees/<name>` (same convention, on the remote fs).

### 3. Remote agent detection (the "Blank Terminal only")
The server already probes remote agent CLIs: `GET /api/hosts/{id}/readiness`
(`host_runtime::readiness` → "every probed agent CLI"). Wire it in:
- Composer Agent picker: when the selected Project is remote (has host_id), build
  the agent list from that host's readiness (installed agent CLIs) instead of the
  local `preflight.detectAgents`. Fall back to local for local projects.
- Likely a small host-readiness client + a branch in the detected-agents/composer
  selection logic keyed on the project's host.

## Sequencing & verification
1. Item 1 (repo host_id) — foundation; no behavior change for local. Verify a
   remote repo round-trips host_id in repos.json.
2. Item 2 create-only first (unblocks the 500): `git worktree add` over SSH →
   verify a worktree appears under the remote `.claude/worktrees`. Then
   list/remove/git-status.
3. Item 3 agent detection — independent of 1/2; can land in parallel.
- Each item: `cargo build` + UI build clean; test against the real Omarchy host
  (the only true validation — remote ops can't be unit-tested without a host).

## Risk / notes
- Cascades: once worktree create is remote, OPEN/stream/status must also be
  host-aware or the worktree looks broken. Do create+list+status together.
- Tilde/quoting: reuse the `resolve`/trailing-slash + shlex patterns already in
  `routes/fs.rs::list_remote_dir`.
- `git_in_dir` over SSH is per-call (a fresh ssh each git op) — fine for
  correctness; optimize with a control-master later if latency bites.
