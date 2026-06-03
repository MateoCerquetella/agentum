# De-Electron the desktop UI: typed Tauri client + implement missing Rust commands

- **Date:** 2026-06-03
- **Status:** IMPLEMENTED (see `2026-06-03-de-electron-PROGRESS.md`)

> **Implementation note (post-build):** the "explicit methods, *no Proxy at all*" goal
> proved unsafe — a static codemod cannot enumerate methods reached via namespace
> aliases / multiline / dynamic access (this caused a renderer crash:
> `remoteWorkspace.clientId is not a function`). Final design keeps the explicit typed
> per-namespace surface **plus** a thin `defineNamespace` fallback (`core.ts`) that
> synthesizes any non-enumerated method/event into the same Tauri command the old
> bridge produced. The *global* `window.api` string-munging contextBridge proxy and the
> `window.electron` shim are still fully deleted — this is a per-namespace safety net,
> not the electron bridge.
- **Scope:** `crates/agentum-desktop` (Tauri shell `src/` + React UI `ui/`)
- **Driver:** User report — "terminal failed to spawn (no pty handle returned)", "Add" not working, clicking a "space" not working. Goal escalated to: remove every Electron bridge, go fully native Tauri, remove all Node-isms, make it work.

## 1. Problem & root cause

The desktop React UI was copied wholesale from an Electron app ("orca", commit `fa0d8ec`). It still talks to a faked Electron `contextBridge` surface (`window.api.*`, `window.electron.*`). On Tauri, `lib/electron-bridge.ts` emulates that surface with a JS `Proxy` (`createApiProxy()`) that string-munges `window.api.X.y(args)` into `invoke('x_y', payload)` and `window.api.X.onZ(cb)` into `listen('x-z', cb)`.

**The reported bugs are NOT bridge bugs.** The proxy routes correctly. The failures are **empty Rust command handlers**:

- `pub fn pty_spawn() {}` — returns nothing → UI's `pty-transport.ts` sees `null`, throws `terminal failed to spawn (no pty handle returned)`.
- `workspace_space_*`, several `worktrees`/`ui` commands — empty stubs.
- **60 empty stub commands out of 465** registered in `src/lib.rs`.

Rewriting the frontend does **not** fix these; implementing the Rust commands does. The two efforts are orthogonal and both are in scope.

## 2. Goals / non-goals

**Goals**
1. Delete the Electron-bridge proxy and the `window.electron` shim; the desktop UI calls Tauri (`invoke`/`listen`) through an explicit, typed client.
2. Remove all Node-isms from the UI (`Buffer.*` ×20, `process.platform` ×14, `process.env` ×3).
3. Implement the empty Rust command stubs that back real features (terminal, workspace-space, Add-workspace, and the rest), fixing the reported bugs.
4. The app builds and runs; reported flows work in the real app.

**Non-goals**
- The **web entry** (`src/web/main.tsx` + `web/web-preload-api.ts`, server-backed `window.api`) is **out of scope**. It is a separate Vite entry, not in the desktop build graph, so it cannot regress the desktop. We leave it as-is (its dangling `PreloadApi` import is pre-existing). We may delete it later if confirmed dead; not in this effort.
- No feature changes, redesigns, or unrelated refactors.

## 3. Verified current state

| Fact | Value |
| --- | --- |
| Desktop entry | `ui/src/main.tsx` → `import './lib/electron-bridge'` |
| Bridge mechanism | `window.api = createApiProxy()` (JS Proxy → `invoke`/`listen`); `window.electron` shim |
| `window.api` type | `any` (`ui/src/env.d.ts`) — **no real typed contract** |
| `PreloadApi` | phantom: imported by web preload from a non-existent `../../../preload/api-types`; tolerated under `strict:false` + separate entry |
| Endpoints | 515 distinct commands + 101 events ≈ 616 |
| Namespaces | ~40 (`ui`, `gh`, `shell`, `pty`, `browser`, `ssh`, `fs`, `git`, `linear`, …) |
| Call sites | 1127 (non-test) |
| Test files mocking `window.api` | 19 |
| Rust commands | 465 registered; **60 empty stubs** |
| Node-isms | `Buffer.` ×20, `process.platform` ×14, `process.env` ×3 |

**Wire mapping the proxy currently uses (must be preserved):**
- Command name: path segments → `segmentToSnakeCase` joined by `_`. e.g. `codexAccounts.list` → `codex_accounts_list`; `pty.spawn` → `pty_spawn`.
- Payload (`argsToPayload`): 1 arg → the arg itself; >1 args → `{ args }`; 0 args → `{}`; non-object single arg → `{ value: x }`.
- Event name (`pathToEvent`): `X.onZ` → segments `[X, Z]` → `segmentToKebabCase` joined by `-`. e.g. `ui.onMaximizeChanged` → `ui-maximize-changed`.
- Event subscribe returns an unsubscribe `() => void`.

## 4. Target architecture

```
ui/src/tauri/
  contract.ts      # AgentumApi interface — the typed contract (the PreloadApi that never existed)
  core.ts          # call(command, payload) over invoke; subscribe(event, cb) over listen; shared types
  pty.ts           # namespace module: explicit methods -> call('pty_spawn', …), subscribe('pty-…', …)
  gh.ts  shell.ts  ssh.ts  fs.ts  git.ts  ui.ts  browser.ts  linear.ts  … (one per namespace)
  index.ts         # assembles modules into `export const api: AgentumApi`
```

- **`contract.ts`** declares `AgentumApi` with one member per namespace; each method typed `(payload) => Promise<T>`, each event typed `(cb: (p: P) => void) => () => void`. Single source of truth; kills the `any`.
- **Namespace modules** implement each member as an **explicit** `call('<command_literal>', payload)` / `subscribe('<event_literal>', cb)`. No Proxy, no runtime name derivation — names are literal strings, greppable and type-checked.
- **`index.ts`** exports a singleton `api: AgentumApi`. Call sites do `import { api } from '@/tauri'` and call `api.pty.spawn(...)`.
- **`core.ts`** centralizes the thin `invoke`/`listen` wrappers and the payload convention, so per-method code stays trivial.

### Migration of call sites
- `window.api.X.y(...)` → `api.X.y(...)` (add `import { api } from '@/tauri'`). Shape preserved ⇒ mostly mechanical.
- Aliases / guards rewritten: `const uiApi = window.api?.ui` → `const uiApi = api.ui`; `window.api?.pet?.import` → `api.pet.import` (always defined now). Behavior preserved (the guards were defending against an absent bridge that no longer exists).

### Deletions
- `ui/src/lib/electron-bridge.ts` (Proxy + `window.electron` shim).
- The `window.electron` consumer(s) (1 non-test use) rewritten to Tauri/`navigator`.
- Node-isms: `Buffer.*` → `Uint8Array`/`TextEncoder`/`atob`/`btoa`; `process.platform` → a single `platform()` helper (Tauri OS plugin or `navigator`); `process.env` → `import.meta.env`.

## 5. Safety invariant — identical wire contract

The explicit command/event name string in every client method **must equal** what the proxy produced, because the Rust side is unchanged.

**Build-time guard (new):** a script/test that (a) extracts the `generate_handler![…]` command list from `src/lib.rs`, (b) extracts every command literal used in `ui/src/tauri/*`, and (c) asserts every client command exists in the Rust handler list (and flags Rust commands the client never calls). Run in CI / pre-merge. This converts the highest risk (name drift) from a runtime surprise into a build failure.

## 6. Always-green incremental rollout

Big-bang is forbidden — the app must build and run at every step.

1. Land `contract.ts` + `core.ts` + `index.ts` + the guard. `api` exists alongside the proxy.
2. **Temporary back-compat:** in `main.tsx`, also assign `window.api = api` so un-migrated call sites keep working during migration.
3. Migrate **namespace-by-namespace** (`pty` first — it backs the reported bug). After each namespace: `tsc` + `vitest` + `vite build` green.
4. When all namespaces are migrated, delete the proxy, the `window.electron` shim, and the temporary `window.api = api` assignment; drop the `window.api` global from `env.d.ts`.

## 7. Rust command implementation

- **Triage the 60 stubs** into: (a) genuine features to implement, (b) **intentional no-ops** (Electron window-chrome Tauri handles natively, e.g. `ui_sync_traffic_lights`, `pty_report_geometry`) — keep as `{}` with a `// no-op on Tauri: <reason>` comment.
- **Implement (a)**, prioritizing the reported flows:
  - `pty_spawn`: parse the options object (`{cols,rows,cwd,env,command,…}`), allocate an id, open the PTY (reuse the `openpty`/reader-thread/writer logic already in `pty_create`), store the handle, return `{ id }` (the `PtyConnectResult` shape the transport expects; reattach/snapshot fields omitted for a fresh spawn).
  - Terminal-lifecycle pty stubs the transport/dispatcher calls (`pty_signal`, `pty_report_geometry`, serializer stubs) — implement or confirm safe no-op per their callers.
  - `workspace_space_*`, Add-workspace (`worktrees`/`ui`) stubs behind the reported flows.
  - Remaining genuine stubs (`ssh_*`, `updater_*`, `speech_*`, `automations_*`, `linear_*`, …).
- Refactor `pty_create`'s inner openpty/reader/writer into a shared helper so `pty_spawn` and `pty_create` don't duplicate it.

## 8. Testing & verification

- **Mockability:** `api` is a plain singleton object → the 19 test files override members (`vi.spyOn(api.pty, 'spawn')` or `vi.mock('@/tauri')`). Update their setup from `window.api = {…}` to module/singleton mocking.
- **Per phase:** `tsc`/oxlint typecheck → `vitest` → `vite build` → `cargo build -p agentum-desktop` → run the app and drive the repaired flow.
- **Guard:** the Rust-handler ↔ client-command assertion (Section 5) must pass.
- **Final acceptance:** launch the desktop app; create a terminal (PTY spawns, shell prompt renders, input echoes); open a workspace "space" (renders/responds); "Add" workspace works; no `window.api`/`window.electron`/Node references remain in `ui/src` (grep clean); `cargo test -p agentum-desktop` and `vitest` green.

## 9. Phased sequence (single cohesive effort)

- **P1 — Foundation:** `contract.ts`, `core.ts`, namespace module skeletons, `index.ts`, back-compat `window.api = api`, the build-time guard.
- **P2 — Migrate call sites** namespace-by-namespace (start `pty`), green at each step. Update test mocks alongside each namespace.
- **P3 — Delete** proxy, `window.electron` shim, node-isms; drop `window.api` global.
- **P4 — Rust:** implement genuine stubs (incl. `pty_spawn`), document intentional no-ops.
- Driven with parallel per-namespace agents during P2/P4.

> Per the chosen direction, there is **no separate early P0 ship** — the terminal is fixed as part of P4 within this sequence. (P4 can be sequenced early in execution to unblock manual testing without changing the cohesive plan.)

## 10. Risks & mitigations

| Risk | Mitigation |
| --- | --- |
| Command/event name drift (515+101) | Build-time guard asserts client names ⊆ Rust handler list (Section 5). |
| Payload-shape mismatch (multi-arg `{args}` cases) | `core.call` reproduces `argsToPayload`; per-namespace tests assert payloads; multi-arg sites are rare (mostly single-object today). |
| 1127-site churn regressions | Always-green namespace-by-namespace; back-compat shim; typecheck+build+tests per step. |
| Test-mock breakage (19 files) | Migrate each test's mock setup in the same namespace step. |
| Rust stub that's actually load-bearing assumed a no-op | Triage reads each stub's callers before deciding implement vs no-op. |
| Scope creep into web entry | Explicit non-goal; web entry isolated from desktop build. |

## 11. Open questions

None — resolved during brainstorming (web out of scope; single cohesive effort, no early P0).
