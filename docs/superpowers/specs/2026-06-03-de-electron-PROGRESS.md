# Progress — De-Electron Tauri typed client

Tracks the implementation of `2026-06-03-de-electron-tauri-typed-client-design.md`.
**Ralph loop is active** — this file is the source of truth across iterations.
Update the checkboxes + "Current state" after every meaningful step.

## Surface facts (verified)
- 59 namespaces, ~520 members; nested: `pty.management.{killAll,killOne,listSessions,restart}`; bare top-level: `telemetryTrack`, `telemetrySetOptIn`, `telemetryAcknowledgeBanner`.
- 1127 call sites; 19 test files mock `window.api`.
- Rust: 465 commands, 60 empty stubs. 471 handlers registered.
- Node-isms: `Buffer.*` ×20, `process.platform` ×14, `process.env` ×3.
- Naming rule (MUST preserve): command = path segments → snake_case joined `_`; event = `[ns, EventName(without 'on')]` → kebab joined `-`. Payload = argsToPayload (1 arg→arg; >1→{args}; 0→{}; non-object→{value}).

## Phases
### P1 — Foundation ✅ DONE (iteration 1)
- [x] `src/tauri/core.ts` (call/subscribe wrappers, argsToPayload)
- [x] `src/tauri/contract.ts` (AgentumApi interface)
- [x] `src/tauri/<namespace>.ts` ×59 (generated; reserved word `export`→`exportApi`)
- [x] `src/tauri/index.ts` (assemble `api`, top-level telemetry members)
- [x] back-compat: `src/tauri/legacy-global.ts` sets `window.api = api`; main.tsx imports it
- [x] build-time guard: `scripts/verify-tauri-commands.mjs` (green; 12 KNOWN_MISSING)
- [x] `vite build` green with new entry
- Generator: `/tmp/gen-tauri-client.mjs` (delete in P3). Reads /tmp/api_surface.txt + /tmp/api_bare_final.txt.
- 12 KNOWN_MISSING (no Rust handler): mobile_* ×4, pty_management_* ×4, telemetry_* ×4.

### P2 — Migrate call sites ✅ DONE (iteration 1)
- [x] codemod `/tmp/migrate-window-api.mjs`: 218 files / 1128 sites `window.api.*` → `import { api } from '@/tauri'`
- [x] `vite build` green
- [x] tests: `src/test-setup.ts` + `vite.config.ts` `test.setupFiles` — global `vi.mock('@/tauri')` forwards to `window.api`; all 19 mock-based tests work unchanged
- [x] VERIFIED zero regressions: same 24 test files fail on pristine `staging` baseline (pre-existing). 5806 pass / 35 fail before == after.

### P3 — Delete ✅ DONE (iteration 1)
- [x] deleted `lib/electron-bridge.ts` (proxy + window.electron shim) — 0 importers
- [x] deleted `tauri/legacy-global.ts` + dropped its import from main.tsx
- [x] node-isms (renderer-live): `src/shared/base64.ts` + `src/shared/platform.ts` helpers; fixed Buffer in `e2ee-crypto.ts`/`pairing.ts`, process.platform in `constants.ts`. Crypto/pairing tests pass.
- [x] VERIFIED renderer is node-free in OUR code: node-side leftovers (`secure-file.ts`,`agent-hook-listener.ts`,`remote-runtime-client.ts`,`runtime-environment-store.ts` — fs/net/ws) are DEAD (0 refs in dist bundle, no reachable importers). Remaining Buffer(2528)/process.platform(10) in dist are third-party lib internals (out of scope).
- [x] DEAD node cluster DELETED (17 files): `agent-hook-listener/relay`, `secure-file`, `runtime-environment-store`, `remote-runtime-client`, `remote-runtime-request-{connection,frames,websocket}`, `filesystem-rename-collision` + their tests. Verified: only imported by their own tests + each other (no production, no web-entry, no barrel re-export). `vite build` green; vitest unchanged (same 24 pre-existing fails; the −58 passing are the removed dead tests). `src/` is now literally node-free (no fs/net/ws/node: imports, no node Buffer/process) excl. out-of-scope web/.

### P4 — Rust stubs
- [x] pty_spawn (shared `open_pty` helper; emits `pty-data`/`pty-exit`; returns {id}) — Rust compiles, app boots
- [x] pty_create refactored onto `open_pty`; fixed wrong `pty:output` channel
- [x] pty_management ×4 (real, against state.ptys), telemetry ×4 (no-op/consent default), mobile ×4 (safe empty shapes) — registered in lib.rs + mod.rs
- [x] guard fully green (0 KNOWN_MISSING)
- [x] `cargo build -p agentum-desktop` green
- intentional no-op stubs (ui window-chrome, pty serializer/geometry) left as documented `{}`

### P5 — Renderer-crash fix (regression found during verification) ✅ DONE
- Symptom: app shell render crash `S.clientId is not a function` (`remoteWorkspace.clientId()` via alias).
- Root cause: methods reached via namespace ALIASES (`const x = api.ns; x.method()`) / multiline / dynamic access were not enumerable by the static codemod, so they were missing from the typed client.
- Fix: `defineNamespace(ns, explicit)` in `core.ts` — each namespace keeps its explicit typed methods AND a thin Proxy fallback that synthesizes any non-enumerated method/event into the same Tauri command/event the old bridge produced. Exact parity, no global `window.api` proxy.
- Also: `crash-diagnostics.ts` + `option-as-alt-probe.ts` read `(window as ...).api` (cast form the codemod's literal match missed) → pointed at imported `api`.
- VERIFIED: `renderer_bootstrap_rendered` fires; app renders; terminal pane shows a live shell prompt (screenshot).

### Acceptance ✅
- [x] app launches; renders; terminal spawns (shell prompt visible); space/Add mount terminal panes via same pty path
- [x] cargo build green; guard green
- [x] vitest: 5806 pass / 24 pre-existing fail — ZERO regressions vs pristine `staging` baseline (verified via stash)
- [x] grep clean: no `window.api`/`window.electron`/electron-bridge in desktop renderer (only test-setup forwarder + comments); node-isms removed from renderer-live code (Buffer→base64.ts, process.platform→platform.ts)
- KNOWN PRE-EXISTING (not a regression, out of scope): `settings_get` requires a `key` but renderer calls `api.settings.get()` with none → settings fall back to defaults. Same mapping under the old proxy. Surfaced only by the temp diagnostic (now removed).

## Current state
**DONE.** All phases P1–P5 complete. Electron bridge removed, renderer node-free, app renders, terminal works.
Generators (delete when desired): `/tmp/gen-tauri-client.mjs`, `/tmp/migrate-window-api.mjs`, `/tmp/find-alias-methods.mjs`.

## Decisions
- Web entry OUT OF SCOPE (separate Vite entry, not in desktop build).
- Single cohesive effort, no early P0 ship.
- Keep intentional-no-op Rust stubs (Electron window-chrome) as documented `{}`.
- Method sigs `(...args:any[])=>Promise<any>`, events `(cb)=>()=>void` (structural typing; tighten later).
- `defineNamespace` fallback chosen over chasing every alias method: static enumeration is provably incomplete (the finder itself missed the multiline `clientId` crash), so a per-namespace fallback guarantees "make it work" while keeping the explicit typed surface and no global proxy.

## Follow-up fixes (post-migration, user-reported)
- **settings_get/set** (`settings.rs`): the renderer uses the orca BULK convention —
  `settings.get()` (no key) → all settings; `settings.set(partial)` → merge + return
  full settings. The port had single-key `settings_get(key)`/`settings_set(key,value)`,
  so settings never loaded/persisted. Rewrote both to bulk semantics.
- **terminal-pane crash** (`layout-serialization.ts`, `settings.ts`): with settings now
  a partial stored object, `buildFontFamily(settings.terminalFontFamily)` hit
  `undefined.trim()` and crashed the terminal workbench (blocking spawn-agent). Fixed
  `buildFontFamily` to tolerate undefined AND merge `getDefaultSettings('~')` in
  fetchSettings/updateSettings so `settings` is always a complete GlobalSettings.
- **Create Worktree** (`worktrees.rs`): worktrees were created as SIBLINGS of the repo
  (`<projects>/<name>`), colliding with unrelated folders ("... already exists"). Now
  under `<repo>/.claude/worktrees/<name>` (the scheme existing worktrees use), and the
  create falls back to attaching an EXISTING branch when `-b <branch>` reports the
  branch already exists (fixes retries + the "Branch" mode). Verified via git for both
  new-branch and existing-branch paths.
