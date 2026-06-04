# PRD — Agentum v2.1: Desktop Thin-Shell Refactor

**Owner:** Mateo Cerquetella
**Status:** Ready to implement
**Target:** Claude Code (agentic coding agent)
**Repo:** https://github.com/MateoCerquetella/agentum
**Supersedes scope of:** PRD-AGENTUM-V2 §6 (Tauri desktop shell) — v2 delivered the shell; this PRD delivers the *thinness*.

---

## 1. Context

Agentum v2 shipped a Tauri 2 desktop app (`crates/agentum-desktop/`) that boots `agentum-server` in-process on a loopback port and drives it over HTTP/WS from a React/Vite webview. The TUI and the desktop now share the same Rust core — that part of v2 is correct.

What v2 *did not* finish: the desktop still ships **parallel native reimplementations** of several server domains alongside their server equivalents. Two domains — `git` and `agents` — were already collapsed to the embedded server (commits `4f995e6` and `b600c26` on `origin/staging`); four more remain (`repos`, `worktrees`, `forge`, `usage`). Several others are placeholder stubs that the UI has never driven end-to-end.

The end state the codebase is reaching for, in one sentence:

> The desktop is a Tauri shell whose Rust side is window/PTY/dialog glue and whose UI side is a thin HTTP/WS client of the embedded `agentum-server` — same core, same SQLite store, one source of truth.

This PRD turns the existing internal roadmap (`docs/superpowers/plans/2026-06-04-desktop-thin-shell-remaining-domains.md`, commit `45528ac`) into a shipping plan with explicit success criteria, an order, and a verifiable done definition per domain.

---

## 2. Non-goals

Explicitly out of scope. Saying these here so the work does not expand:

- **No new domains.** Not porting `pty`, `window`, `fs`, `shell`, `clipboard`, `keybindings`, `browser`, `speech`, `notifications`, `app`, `updater`, `diagnostics`, `settings` (the local `~/.agentum/Agentum/settings.sqlite3` KV), `accounts` (Claude/Codex account switching), or `telemetry` to the server. They are OS glue or state that the server deliberately does not mirror.
- **No new UI features.** This PRD ships no new buttons, no new panels, no new keyboard shortcuts. If a domain today shows a placeholder, the placeholder stays until a later PRD builds the real surface.
- **No removing the `embedded_loopback` indirection.** The shell-to-server boundary stays loopback HTTP/WS. No Unix socket, no shared in-process call, no FFI shortcut.
- **No desktop-mobile divergence.** Mobile (Tauri 2 iOS/Android) is a future PRD; this one is desktop only.
- **No workspace `Cargo.toml` reshuffle beyond dep hygiene.** We do not split or merge crates.
- **No migrating the desktop to `directories`.** `dirs = "6"` stays; switching APIs is a separate, higher-risk change.

---

## 3. Success criteria

The refactor is **done** when all of the following hold:

1. **Single backend per domain.** Every user-facing surface in the desktop UI is served by exactly one backend — either a Tauri command (for OS-glue) or a route on the embedded `agentum-server`. No domain has both.
2. **No dead Rust crates in `agentum-desktop/Cargo.toml`.** A `cargo machete` / `cargo udeps` clean run on the desktop crate shows zero unused dependencies.
3. **No native `git2`.** Verified by `grep -R "git2::" crates/agentum-desktop/src` returning zero hits and `git2` absent from `crates/agentum-desktop/Cargo.toml`.
4. **Workspace dep discipline.** `crates/agentum-desktop/Cargo.toml` uses `{ workspace = true }` for every dep that exists in `[workspace.dependencies]`. Direct versions only for desktop-only deps.
5. **TUI ↔ desktop parity preserved.** Every action available in the TUI is reachable in the desktop, and the response is byte-identical at the HTTP/WS boundary. Verified by running the TUI and the desktop against the same embedded server and diffing the JSON envelopes for a fixed session.
6. **Smoke test checklist per domain passes.** Each of the four remaining domains has an executable checklist (see §7) that the implementer can run on a real desktop in under five minutes.
7. **Binary size delta ≤ 5%.** The release `agentum-desktop` bundle (`.dmg`/`.AppImage`/`.msi`) does not grow by more than 5% versus v2.10.x. The point is *removing* code, not adding it.
8. **`cargo test --workspace --lib` green.** No new test failures, no skipped tests, no `#[ignore]` without a comment.

---

## 4. Architecture: the target shape

```
crates/
├── agentum-core/         # unchanged: Session, Status, Event, transcript types
├── agentum-store/        # unchanged: SQLite (sqlx) repository
├── agentum-tmux/         # unchanged: tmux CLI wrapper
├── agentum-watchdog/     # unchanged: tail loop, event emission
├── agentum-executor/     # unchanged: ToolAdapter trait + per-agent argv
├── agentum-server/       # EXPANDS: gains routes/repos.rs, routes/worktrees.rs,
│                         #          routes/forge.rs extensions, usage aggregation
└── agentum-desktop/      # THINS:
                          #   src/lib.rs  — setup hook boots embedded server, manages state
                          #   src/commands/server.rs — exposes loopback URL
                          #   src/commands/<os-glue>.rs — Tauri commands for native-only concerns
                          #   (NO commands for: git, repos, worktrees, forge, usage, agents,
                          #    preflight — those live in the server)
```

### 4.1 Boundary contract

The desktop shell **may** keep Tauri commands only for the following surface categories (each tied to a Tauri plugin or a native OS API that has no server equivalent):

| Category | Reason it stays native |
|---|---|
| Window / chrome | `tauri::Window` lifecycle, `tray-icon`, traffic-light sync, zoom level, close confirm |
| Local PTY | `portable-pty` + native reader thread → "pty-data"/"pty-exit" events for the renderer's `xterm.js` |
| Filesystem dialogs | `tauri-plugin-dialog` (folder picker, file picker, save dialog) — must run in the webview process |
| Filesystem watch | `notify` on the worktree root — must watch the local FS, not the server's |
| Clipboard | `tauri-plugin-clipboard-manager` (image + text round-trip) — webview-origin restriction |
| Shell open | `tauri-plugin-shell` (open URL, open file URI, reveal in file manager) — `ShellExecute`/LaunchServices |
| Native notifications | `tauri-plugin-notification` + `notifications_play_sound` (browser autoplay policy) |
| SSH client | Local `ssh`/`ssh-add` integration, keychain passthrough, port-forward management |
| Settings KV | Local SQLite at `~/.agentum/Agentum/settings.sqlite3` — server has a 2-field `preferences` route that does not mirror this |
| Accounts | Live OAuth flows for Claude/Codex account add/select/reauth — auth dance must run locally |
| Telemetry | Local sampling, opt-in, bundle upload — runs in the shell process |
| Crash reports | Local breadcrumbs, local disk bundle, deferred upload |
| Speech-to-text | Local model download, microphone capture — system-level audio device |
| Updater | Tauri updater plugin → GitHub release `latest.json` |
| Browser automation | Embedded Chromium driver for the `browser` pane (host-runtime feature) |

### 4.2 What goes to the server

The **single source of truth** for the following is `agentum-server`. The desktop is a thin client.

| Domain | Server route | Replaces |
|---|---|---|
| Repos registry | `routes/repos.rs` | `commands/repos.rs` CRUD (349 lines) |
| Worktrees registry + git ops | `routes/worktrees.rs` | `commands/worktrees.rs` (500 lines) |
| Forge (gh + gl) | `routes/forge.rs` (extend) | `commands/gh.rs` (266) + `commands/gl.rs` (91) |
| Usage analytics | `routes/usage.rs` (extend) | `commands/{claude,codex,open_code}_usage.rs` (3× ~70) |
| Git (already done) | `routes/sessions.rs` git subroutes | `commands/git.rs` (deleted in `4f995e6`) |
| Agent detection (already done) | `routes/preflight.rs` | `commands/agents.rs` (deleted in `b600c26`) |

### 4.3 Why this is the right shape

- **One SQLite.** The desktop and the TUI already share `agentum-store::open_default()`. Folding the registries (`~/.agentum/repos.json`, `~/.agentum/worktrees.json`) into the server's store means one backup target, one migration path, one query surface.
- **One watchdog.** The server's `agentum_watchdog::run_session_comment_bridge` and the host-metrics ticker are already wired. Moving repos/worktrees there gets free event broadcasting and the existing WS bus.
- **One set of git argv.** `run_git` + `cwd_for` in the server already handle `--git-dir`, env var sanitization, and the hardcoded `--set-upstream origin HEAD` push. Repos and worktrees both go through it.
- **Testability.** Pure aggregation in `usage` (JSONL → summary/daily/breakdown) is testable headless. The TUI-only agent on the agent's host can do `cargo test -p agentum-server` without a running desktop.
- **Crash surface shrinks.** Removing `git2`, the `rusqlite` settings KV (kept native — see boundary), the `notify` watcher, the `base64` of native git blobs, the `chrono` of git history, etc. means the desktop binary stops growing.

---

## 5. The proven pattern: replication recipe

The `4f995e6` and `b600c26` commits prove a six-step recipe. Every domain in this PRD follows it, in order, no skipping:

1. **Server route.** Build `crates/agentum-server/src/routes/<dom>.rs`. Reuse `run_git` / `cwd_for` / `agentum_store` plumbing. Faithfully port the native logic so the result matches the native output byte-for-byte (preserving JSON key order, `extra` flattening, trailing newline, etc.). Register in `lib.rs::router()`.
2. **Rust unit tests.** Test the pure logic (parsing, selection, registry CRUD, lineage resolution). Verify with `cargo test -p agentum-server` (note: the workspace `cargo test` target is currently red from `hook_base` work — verify pure fns standalone via `rustc --test` or a scratch binary until that lands).
3. **Server client.** Add `ui/src/runtime/server-<dom>-client.ts` (typed `getJson` / `postJson` over `apiUrl()` from `server-endpoint.ts`).
4. **Runtime wrapper.** Add `ui/src/runtime/runtime-<dom>-client.ts` mirroring `runtime-git-client` Slice A: for reads, `serverRead(server, local)` — try server first, fall back to the local registry/JSON. For writes, prefer server, fall back to native during the de-dup window.
5. **Verify.** `cargo check -p agentum-server -p agentum-desktop` + `vite build` + `tsc` (symlink `crates/agentum-desktop/shared → ui/src/shared` first — see CLAUDE.md) + `vitest src/runtime/`. All green.
6. **Smoke test on a running desktop.** The implementer runs the per-domain checklist in §7. Only after the checklist passes, proceed.
7. **Delete the native.** Remove the Tauri command module, the `tauri/<dom>.ts` wrapper, the `contract.ts` namespace entry, the `invoke_handler` registration. Re-run the verify step.

The native fallback stays in the runtime wrapper until step 6. The wrapper makes the switch a one-line change: `serverRead` only.

---

## 6. Phase plan

Strict order. Each phase is independently shippable; ship the tag at the end of each.

### Phase A — Hygiene (one commit, no new features)

**Scope:** `crates/agentum-desktop/Cargo.toml` dep cleanup. The desktop is the only crate that does not use `{ workspace = true }` consistently (7 of ~30 deps; the server uses 23, the CLI uses 24).

**Tasks:**

- Replace direct versions with `{ workspace = true }` for: `serde`, `serde_json`, `tokio`, `anyhow`, `thiserror`.
- Document the `dirs` vs `directories` decision: keep `dirs = "6"` (different API; migration is out of scope).
- After `git pull origin staging`, remove `git2 = "0.20"` (orphan post-`4f995e6`).
- Run `cargo machete` (or `cargo udeps`) and resolve any flagged deps.

**Acceptance:**

- `cargo check -p agentum-desktop` green.
- `cargo machete` clean.
- Desktop `Cargo.toml` line count drops by ≥ 6.

**Tag:** none (commit on the staging branch, no version bump).

### Phase B — Repos registry (`routes/repos.rs`)

**Scope:** Move the 349-line `commands/repos.rs` (registry at `~/.agentum/repos.json`, CRUD, three git-ref helpers) to the server. Keep the two dialog commands (`repos_pick_folder`, `repos_pick_directory`) native.

**Server route surface:**

```
GET    /api/repos
POST   /api/repos                  # add existing
PATCH  /api/repos/{id}             # update meta (displayName, badgeColor, extra)
DELETE /api/repos/{id}
POST   /api/repos/reorder          # { order: [id, ...] }
POST   /api/repos                  # create new (git init + folder)
POST   /api/repos/clone            # async clone with progress events
POST   /api/repos/clone-abort
GET    /api/repos/{id}/base-ref-default
GET    /api/repos/{id}/base-refs?q=&limit=
GET    /api/repos/{id}/base-refs/details
POST   /api/repos/{id}/remote      # add remote (SSH-keyed, error variant)
```

**Native kept:** `repos_pick_folder`, `repos_pick_directory` (dialog), `repos_clone_abort` no-op (or move to server).

**Registry format invariant:** the JSON at `~/.agentum/repos.json` must round-trip byte-identically (key order, `extra` object shape, trailing newline, UTF-8 BOM if present). The existing `read`/`write` helpers in the native file become the server's helpers verbatim, just relocated.

**Server-client:** `server-repos-client.ts` with typed shapes. `runtime-repo-client.ts` extends to wrap the three git-ref helpers (smallest, read-only, lowest risk). CRUD wrapper added in a second pass.

**Smoke test checklist (B):**

- [ ] Open the app — project list renders from server.
- [ ] "Add Folder" → dialog opens (native) → folder added, appears in list.
- [ ] Rename a project → displayName persists across reload.
- [ ] Reorder → order persists across reload.
- [ ] "New Project" (create + git init) → appears in list, `.git/` exists.
- [ ] "Remove" → gone from list, disk folder untouched.
- [ ] New-Session / New-Worktree dialog base-ref autocomplete: server route responds under 200ms with 10 results.
- [ ] Kill the desktop, restart, verify list is intact (proves one source of truth on disk).

**Risk:** **HIGH.** This is the live project list; a bad write corrupts the user's workspace index. Port the existing native logic verbatim; do not "improve" while moving.

**Tag:** `v0.10.11`.

### Phase C — Worktrees (`routes/worktrees.rs`)

**Scope:** Move the 500-line `commands/worktrees.rs` (registry at `~/.agentum/worktrees.json` + sort-order file + git-worktree CLI ops) to the server. Reconcile with the server's existing per-session worktree creation (`createSession{worktree:true}`) so a worktree is not tracked in two places.

**Server route surface:**

```
GET    /api/worktrees?repoId=
GET    /api/worktrees/all
GET    /api/worktrees/detected
POST   /api/worktrees              # create (git worktree add)
PATCH  /api/worktrees/{id}         # update meta (pin, label, etc.)
DELETE /api/worktrees/{id}         # git worktree remove
POST   /api/worktrees/{id}/force-delete-preserved-branch
GET    /api/worktrees/lineage?repoId=
PATCH  /api/worktrees/lineage
POST   /api/worktrees/sort-order
GET    /api/worktrees/resolve-pr-base?repoId=&base=
```

**Native kept:** none from this module — all worktree ops are server-side.

**Dependency:** Phase B (worktree routes resolve `repoId` against the repos registry).

**Server-client:** `server-worktrees-client.ts`. `runtime-worktrees-client.ts` mirrors the git Slice A pattern.

**Smoke test checklist (C):**

- [ ] Create worktree from a base ref — appears in sidebar, `git worktree list` shows it on disk.
- [ ] Pin / unpin — `pinned` flag persists across reload.
- [ ] Rename / recolor — meta updates, no git mutation.
- [ ] Remove worktree — disappears from sidebar; branch preserved on disk (unless force-delete was used).
- [ ] Force-delete preserved branch — branch gone from `git branch -a`.
- [ ] Detected-worktree list (worktrees on disk not in the registry) — shows them, "Import" works.
- [ ] Sort order — drag-to-reorder persists across reload.
- [ ] Lineage view — parent/child relationships render correctly.

**Risk:** **VERY HIGH.** Destructive git-worktree operations on the user's real trees. Port the existing native logic verbatim (the 33 git operations enumerated in the deleted `commands/git.rs` and the worktree-specific logic in `commands/worktrees.rs`). Do not "improve" while moving.

**Tag:** `v0.10.12`.

### Phase D — Forge (gh + gl) (`routes/forge.rs` extension)

**Scope:** The server has 5 session-scoped forge endpoints (`/api/sessions/{id}/forge/{info,prs,pr,issues,checks}`) and `/api/forge/token`. The desktop's `commands/gh.rs` (266 lines, 30 commands) and `commands/gl.rs` (91 lines) are mostly **arg-less stubs** returning placeholder JSON today. This phase:

1. Builds the **read-only** surface in the desktop UI: list PRs, list issues, view checks, view PR details. Routes through the server. Keeps native fallback until smoke-tested.
2. Builds the **mutation** surface: merge, auto-merge, request reviewers, update labels, project items, comments, reviews. One route per mutation, gated by the session-scoped auth token.
3. Adds the missing ~25 endpoints in `routes/forge.rs`. Token acquisition via `/api/forge/token` (already exists).

**Server route surface (additions):**

```
POST   /api/sessions/{id}/forge/prs/{n}/merge             { method, sha? }
POST   /api/sessions/{id}/forge/prs/{n}/auto-merge       { enable }
POST   /api/sessions/{id}/forge/prs/{n}/reviewers         { reviewers }
PATCH  /api/sessions/{id}/forge/prs/{n}                   { title, body, state, base, draft }
POST   /api/sessions/{id}/forge/prs/{n}/checks/{id}/rerun
POST   /api/sessions/{id}/forge/prs/{n}/comments
POST   /api/sessions/{id}/forge/prs/{n}/review-comments  { path, line, body, side?, in_reply_to? }
POST   /api/sessions/{id}/forge/prs/{n}/review-threads/{id}/resolve
GET    /api/sessions/{id}/forge/issues
POST   /api/sessions/{id}/forge/issues                   { title, body, labels, assignees }
PATCH  /api/sessions/{id}/forge/issues/{n}               { state, labels, assignees, title, body }
POST   /api/sessions/{id}/forge/issues/{n}/comments
DELETE /api/sessions/{id}/forge/issues/{n}/comments/{cid}
PATCH  /api/sessions/{id}/forge/issues/{n}/comments/{cid}
GET    /api/sessions/{id}/forge/projects                 # GitHub Projects v2
GET    /api/sessions/{id}/forge/projects/{id}/views
GET    /api/sessions/{id}/forge/projects/views/{id}/items
PATCH  /api/sessions/{id}/forge/projects/items/{id}      { field, value }
DELETE /api/sessions/{id}/forge/projects/items/{id}/field/{field}
GET    /api/sessions/{id}/forge/work-items
GET    /api/sessions/{id}/forge/accessible-projects
POST   /api/sessions/{id}/forge/resolve-project-ref
GET    /api/sessions/{id}/forge/issue-types
GET    /api/sessions/{id}/forge/labels
GET    /api/sessions/{id}/forge/assignable-users
```

And the GitLab mirror set in `routes/forge.rs` under a `?provider=gl` discriminator (or split to `routes/forge_gl.rs` if the divergence is large; decide at the start of the phase).

**Native deleted:** all of `commands/gh.rs` and `commands/gl.rs` once their server routes pass smoke.

**Server-client:** `server-forge-client.ts` (typed by `provider: 'github' | 'gitlab'`). `runtime-forge-client.ts` routes to the **bound session** (`ensureWorkspaceSession` — already exists) before calling.

**Smoke test checklist (D):**

- [ ] PR list renders for the bound session.
- [ ] PR detail view: checks, reviews, comments, files all load.
- [ ] Merge a draft PR → success path.
- [ ] Merge a non-mergeable PR → error surfaces, no mutation.
- [ ] Request reviewers → reviewers appear on the PR.
- [ ] Issue create → appears in list, has correct labels.
- [ ] Issue comment add/delete → list updates.
- [ ] GitLab: list MRs, view MR, resolve discussion, retry job, close/reopen/merge MR.
- [ ] Token flow: invalid token → 401, refresh path works.

**Risk:** **MEDIUM-HIGH.** OAuth/token handling is fragile. Do reads first behind fallback, verify token round-trip, then mutations.

**Tag:** `v0.10.13`.

### Phase E — Usage analytics (`routes/usage.rs` extension)

**Scope:** The server has `/api/usage` (sidebar chip) and `/api/usage/claude` / `/api/usage/codex` (plan-limit %). The desktop panes want **per-day / per-model / per-project / recent-sessions** aggregations. This phase builds the aggregation in `crates/agentum-server/src/usage.rs`:

- Parse `~/.claude/projects/<project>/*.jsonl` and `~/.codex/sessions/<date>/*.jsonl`.
- Compute summary (today / 7d / 30d tokens + cost), daily series, breakdown by model, recent sessions list.
- Cost needs a model-price table (built-in, refreshed quarterly via a `pricing.json` shipped with the binary).

**Server route surface (additions):**

```
GET /api/usage/scan-state          # { claude: { scannedAt, path, fileCount }, codex: ..., opencode: ... }
POST /api/usage/enable             # { provider: 'claude' | 'codex' | 'opencode', enabled: bool }
POST /api/usage/refresh            # async, emits progress via events bus
GET /api/usage/summary             # { claude: {...}, codex: {...}, opencode: {...} }
GET /api/usage/daily               # { claude: [...], codex: [...], opencode: [...] }  per-day series
GET /api/usage/breakdown           # by model: tokens, cost
GET /api/usage/recent-sessions     # last N sessions with cost + duration
```

**Native deleted:** all of `commands/claude_usage.rs`, `commands/codex_usage.rs`, `commands/open_code_usage.rs`.

**Testability:** this is the **only** domain with substantial headless-testable logic. Aggregation is pure: file → parsed records → aggregated shapes. Unit-test with fixture JSONL files in `crates/agentum-server/src/usage.rs#[cfg(test)]`.

**Server-client:** `server-usage-client.ts`. No `runtime-usage-client.ts` needed — the panes are server-only consumers (no local data source to fall back to).

**Smoke test checklist (E):**

- [ ] Scan state reports correct path, file count, last-scanned timestamp.
- [ ] Refresh a real `~/.claude/projects` directory with N sessions → summary, daily, breakdown all populate.
- [ ] Cost numbers reconcile to a hand-calculation on a known single-session fixture (≤ 1% drift).
- [ ] Recent-sessions list shows the most recent N in correct order.
- [ ] `opencode` provider with empty path → zeroed shape, no error.
- [ ] Refresh a codex session directory → summary, daily, breakdown all populate.
- [ ] TUI usage chip still works (regression — TUI uses `/api/usage`, not the new endpoints; both must coexist).

**Risk:** **MEDIUM.** Cost/model accounting is the riskiest piece. Build with a small fixture first; validate against a real `~/.claude` directory before shipping.

**Tag:** `v0.10.14`.

### Phase F — Boundary documentation (no code change)

**Scope:** Update `CLAUDE.md` with an explicit "Desktop boundary" section that enumerates the 15 categories in §4.1 and lists the 4 categories that the server owns (repos, worktrees, forge, usage) plus the 2 already done (git, agents). This is a documentation commit only.

**Acceptance:** A new contributor reading `CLAUDE.md` can answer "is `<X>` native or server-side?" without reading the codebase.

**Tag:** none (commit on staging).

---

## 7. Cross-cutting technical decisions

### 7.1 Server-route auth model

The desktop embeds the server with `state.no_auth = true` (loopback bind, no bearer token). Server routes added in this PRD therefore do **not** require token validation when called from the embedded instance. If a route is later exposed to remote clients (Phase 2 of the v2 PRD — agentless SSH), the `require_token` middleware applies automatically via `is_public` / `routes::router` wiring. No per-route decisions needed.

### 7.2 Event broadcasting

Routes added in this PRD should emit on the existing `tokio::sync::broadcast` event bus for state changes (`Event::RepoAdded`, `Event::WorktreeCreated`, etc.) so the desktop and TUI clients receive updates over the existing `/api/events` WS. The `Event` enum lives in `agentum-core`; additions go there with a serde rename and a stable variant ordering.

### 7.3 Migration of registry data on first read

When a route reads `~/.agentum/repos.json` or `~/.agentum/worktrees.json` for the first time, it must be tolerant of the legacy format (no `extra`, no `connectionId`). Use `#[serde(default)]` on every field. Do not migrate the file in place on read — write only on the next mutation, and only if the format actually changed.

### 7.4 Native fallback semantics

The runtime wrapper (`runtime-<dom>-client.ts`) follows Slice A of `runtime-git-client`:

```ts
// Reads
export async function readX(...args): Promise<X> {
  return serverRead(server, () => localRead(...args), 'desktop.runtime.x')
}

// Writes
export async function writeX(...args): Promise<void> {
  return serverWrite(server, () => localWrite(...args), 'desktop.runtime.x')
}
```

The `serverRead` / `serverWrite` helpers log a warning on fallback and increment a counter (visible in `runtime_get_status`). The fallback is removed when the domain's smoke test passes.

### 7.5 What we do not do

- No per-domain binary toggles (one compile flag per domain is a maintenance trap).
- No dynamic plugin loading (Tauri's plugin system is for OS glue, not server features).
- No WASM-compiled server routes (premature; the server is already in-process).
- No in-process FFI shortcut (the embedded loopback is the contract; bypassing it breaks the TUI parity guarantee).
- No rewriting registries into SQLite (they stay as JSON files for trivial backup/inspect; the server just owns the read/write).

---

## 8. Implementation order

Strict order. Do not start phase N+1 before phase N is shipped and tagged.

1. **Phase A (hygiene).** Commit on `staging`. No tag.
2. **Phase B (repos).** Tag `v0.10.11`. Ship to internal users.
3. **Phase C (worktrees).** Tag `v0.10.12`. Ship to internal users.
4. **Phase D (forge reads, then mutations).** Read endpoints behind fallback; mutations only after reads pass. Tag `v0.10.13`. Ship to internal users.
5. **Phase E (usage).** Aggregation + unit tests first; routes second; desktop wiring third. Tag `v0.10.14`. Ship to internal users.
6. **Phase F (boundary docs).** Commit on `staging`. No tag.
7. **Tag `v0.11.0`** as the "thin-shell" release. Post on r/selfhosted and the project's release notes.

Each phase ships independently. If phase D drags, phases B and C are still shippable.

---

## 9. Risks and mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Registry format drift corrupts user data on first read | Medium | High | `#[serde(default)]` everywhere; byte-identical round-trip test; manual smoke test on a real `~/.agentum/repos.json` |
| Git argv divergence between native and server | Low | High | `run_git` + `cwd_for` already used by session git; do not introduce a second argv builder; snapshot a known push/pull/fetch and diff |
| Native fallback masks server bugs | Medium | Medium | `serverRead` logs a `console.warn` on fallback; counter exposed in `runtime_get_status`; smoke test must run on the server-only path |
| `usage` cost model diverges from real billing | Medium | Medium | Show "estimate" badges in the UI; reconcile against Anthropic's `GET /api/oauth/usage` for Claude (already wired) and OpenAI's dashboard for Codex; refresh pricing quarterly |
| OAuth token mishandling in forge mutations | Medium | High | Use the existing `/api/forge/token` flow; never log tokens; never store them in the worktree metadata; revoke on session end |
| Phase C breaks the bound-session worktree invariant | Medium | High | Reconcile with `createSession{worktree:true}` upfront; add a route that returns whether a path is a known worktree before `git worktree add` |
| Embedded-server loopback port collision | Low | Low | The OS picks an ephemeral port (`bind((LOCALHOST, 0))`); the `setup` hook stores the result; the desktop reads it via `app_get_server_endpoint`. No change needed |
| TUI regressions from the new server routes | Low | High | `cargo test --workspace --lib` covers the server; TUI smoke test (run `agentum terminal` against a `serve_embedded_loopback` instance) is the final gate before tag |

---

## 10. Out-of-band notes for the implementing agent

- **Match surrounding style.** `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` is green today; do not regress it.
- **Comments encode why, not what.** CLAUDE.md is explicit on this.
- **Tests live next to the code they cover.** No new test crate; no shared fixtures across crates. Per-domain `#[cfg(test)] mod tests` only.
- **No new third-party crates** without asking. The server's dep list is already long; if a new dep is genuinely needed (e.g., a JSONL streaming parser), justify it in the commit message.
- **Smoke tests are not optional.** Each phase has a checklist in §6. The implementer runs the checklist, attaches the output to the PR, and the reviewer signs off before the native is deleted.
- **The native fallback is deleted last, not first.** Resist the urge to "just remove the command file." The fallback is what lets us ship incrementally.
- **Do not split or rename crates** as part of this work. The crate map in `CLAUDE.md` is the source of truth.
- **TUI parity is non-negotiable.** Every server route added here must be reachable from the TUI over the same `agentum-server` (the TUI already speaks the same protocol). If a route is desktop-only, justify it in the commit.

---

## 11. Open questions to answer before starting Phase B

1. **Repos registry atomic write.** The current native code writes `repos.json` via `std::fs::write` (non-atomic). A crash mid-write corrupts the index. Atomic write (write-temp + rename) is a strict improvement, but it changes the on-disk format invariant in §6 Phase B. **Recommend: keep non-atomic write in the port (byte-identical), add an explicit Phase G follow-up for atomic write with backup-restore recovery.** Answer: yes/no.
2. **Worktree sort-order file.** `worktree-sort-order.json` lives next to `worktrees.json` in `~/.agentum/`. Should it move into the server route or stay as a side file? **Recommend: keep as a side file in the port (no schema change); fold into a unified `worktrees` registry in a future PRD.** Answer: yes/no.
3. **Forge token storage.** The desktop currently has no token persistence. The server's `/api/forge/token` flow returns a token per request. Should the desktop cache it for the session, or refetch on every call? **Recommend: cache in memory only, refetch on `app_get_server_endpoint` boot.** Answer: yes/no.
4. **Usage pricing refresh cadence.** The model-price table is shipped in the binary. Quarterly refresh is the plan. **Recommend: ship the first table in `v0.10.14`; a separate PRD adds a `pricing.json` release artifact and a `/api/usage/pricing` route.** Answer: yes/no.
5. **Per-workspace vs global repos/worktrees registry.** The current native code is global (one `~/.agentum/repos.json`). The server's `agentum_store` is SQLite and could scope these per-workspace. **Recommend: keep global in the port (one source of truth on disk, byte-identical to native); scope-by-workspace is a separate PRD.** Answer: yes/no.

End of PRD.
