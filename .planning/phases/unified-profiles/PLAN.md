# Unified Profiles: shared source of truth between TUI and dashboard

## Goal

The TUI's `~/.config/agentum/profiles.toml` becomes the single source of
truth for connection profiles. The dashboard reads/writes the same list
via a new `/api/profiles` endpoint, replacing its localStorage-backed
store. Adding a profile in the TUI shows up in the dashboard on next
load and vice-versa.

## Non-goals

- **Token unification.** TUI tokens stay in `credentials.toml` keyed by
  `host:port`; dashboard tokens stay in browser storage. Reasons:
  - Exposing credentials via API means daemon A can hand out daemon B's
    bearer tokens to whoever can authenticate to A — security smell.
  - Browser tokens are scoped to the browser/device anyway; copying
    them onto disk doesn't help cross-device sync.
  - Profile *metadata* (URL, fingerprint, label) is what users want to
    avoid retyping. Tokens get re-earned on login per device.
- **Cross-daemon sync.** Each daemon owns its own `profiles.toml`.
  Loading the dashboard from `https://my-vps:8822/` shows the VPS's
  profiles, not the local laptop's. That's the right semantics.
- **Multi-user awareness.** The daemon is single-user today; profiles
  are per-OS-user-account because the file lives in `$XDG_CONFIG_HOME`.

## Architecture decision

Today:

```
TUI         ──reads/writes──► ~/.config/agentum/profiles.toml
Dashboard   ──reads/writes──► localStorage (per browser)
```

After:

```
TUI         ──reads/writes──► ~/.config/agentum/profiles.toml ◄─┐
Daemon      ──reads/writes──► same file ◄───┐                   │
Dashboard   ──HTTP /api/profiles──► Daemon ─┘                   │
                                                                 │
(token still lives separately: credentials.toml on TUI side, ───┘
 per-profile in browser on dashboard side)
```

The `Profiles` struct moves out of `crates/agentum/src/commands/terminal/profiles.rs`
into a place both the daemon and the CLI binary can import.

## Tasks

### 1. Move `Profiles` to a shared crate

**Where:** `crates/agentum-core/src/profiles.rs` (new file).

`agentum-core` already holds shared types and depends on nothing
heavyweight. Choosing it over `agentum-store` because profiles are
filesystem-backed, not SQLite-backed.

- Move the entire contents of
  `crates/agentum/src/commands/terminal/profiles.rs` to
  `crates/agentum-core/src/profiles.rs`.
- Re-export from `crates/agentum-core/src/lib.rs`:
  `pub mod profiles; pub use profiles::{Profile, Profiles, ProfilesFile};`
- Replace the original file with `pub use agentum_core::profiles::*;`
  so existing TUI call sites in `crates/agentum/src/commands/terminal/*.rs`
  and `crates/agentum/src/commands/profiles.rs` keep compiling.
- The TOML serde format stays identical — wire compatibility with
  every existing `profiles.toml` on disk is non-negotiable.

**Commit:** `refactor: move Profiles into agentum-core for daemon access`

### 2. Add `/api/profiles` REST routes

**Where:** `crates/agentum-server/src/routes/profiles.rs` (new file),
follows the shape of `routes/notes.rs`.

Endpoints:

| Method | Path                        | Body                | Returns                |
|--------|-----------------------------|---------------------|------------------------|
| GET    | `/api/profiles`             | —                   | `{default, profiles}`  |
| POST   | `/api/profiles`             | `{name, profile}`   | `201` + created entry  |
| PUT    | `/api/profiles/{name}`      | `Profile`           | updated entry          |
| DELETE | `/api/profiles/{name}`      | —                   | `204`                  |
| PUT    | `/api/profiles/default`     | `{name: String?}`   | `204`                  |

Implementation notes:

- Each handler does `Profiles::load()?`, mutates, drops. The file is
  small and rewritten atomically — no need for an in-memory cache.
- TUI mutations are *not* notified to the dashboard via WebSocket in
  this phase. Refresh-on-next-load is acceptable. (See "Future work"
  for the optional broadcast hook.)
- Validation: reuse the existing `is_valid_name` from `Profiles`.
- Errors map cleanly to `ApiError`: bad name → 400, missing →
  404, IO failure → 500.

Register the router in `crates/agentum-server/src/lib.rs` next to
`notes::router()`. Auth middleware applies automatically (no entry in
`auth::is_public`).

**Tests** (in `routes/profiles.rs::tests`):
- Round-trip create → list → update → delete.
- Invalid name → 400.
- DELETE on missing profile → 404.

**Commit:** `feat(server): expose Profiles via /api/profiles REST`

### 3. Dashboard: API-backed profile store

**Where:** rewrite `dashboard/src/lib/profiles.ts`.

The public API stays the same — `profiles` writable, `activeProfileId`
writable, `apiUrl`, `wsUrl`, `getActiveProfile`, `upsertProfile`,
`removeProfile`, `setActiveProfile`. The persistence layer changes.

Changes:

- **Load:** on module init, fire `GET /api/profiles` against the
  *page origin* (not via `apiUrl` — we're bootstrapping). Hydrate the
  `profiles` store with the response. While the request is in flight,
  fall back to localStorage so the UI doesn't flash empty.
- **Save:** on `upsertProfile` / `removeProfile`, fire the matching
  HTTP call. Optimistically update the local store and roll back on
  failure (toast the error).
- **Token storage:** keep `Profile.token` in localStorage exactly as
  today. The daemon never sees it. The new GET/POST omit `token` from
  the wire format — server-side struct has no `token` field, only
  `url`, `fingerprint`, `insecure`.
- **Active id:** stays in localStorage (`agentum_active`). It's a UI
  preference, not server state.

**The "served by remote daemon" case:** when the user loads
`https://my-vps:8822/`, the GET hits the VPS's profile list. That's
fine and expected. The local profile entry won't appear unless the
user POSTed it to the VPS too — also fine, you don't want your
laptop's loopback profile visible on a shared server.

**Migration:** on first load after upgrade, if `localStorage`
contains a `agentum_profiles` array but `GET /api/profiles` returns
an empty list, POST each local entry up to the daemon (best-effort,
ignore conflicts). Then keep using the API. Don't delete localStorage
— users may downgrade.

**Commit:** `feat(dashboard): back profile store with /api/profiles`

### 4. Wire types & contracts

**Where:** `dashboard/src/lib/api.ts`.

Add typed wrappers next to the existing `request()` helper:

```ts
listProfiles(): Promise<{default: string|null, profiles: Record<string, ServerProfile>}>
createProfile(name: string, p: ServerProfile): Promise<ServerProfile>
updateProfile(name: string, p: ServerProfile): Promise<ServerProfile>
deleteProfile(name: string): Promise<void>
setDefaultProfile(name: string|null): Promise<void>
```

Where `ServerProfile` mirrors the Rust `Profile` (url, fingerprint?,
insecure) — no token field.

The dashboard's existing `Profile` type keeps `token` and `id`/`label`
locally; reconcile with `ServerProfile` in `profiles.ts`:
- `id` ↔ server's map key (the name)
- `label` derives from `id` if not stored separately. Stash labels in
  localStorage keyed by name — labels are pure UX, no need to round-trip.
- `baseUrl` ↔ `url`
- `token` stays local-only.

**Commit:** included in step 3.

### 5. EndpointSwitcher + onboarding wiring

**Where:**
- `dashboard/src/lib/components/EndpointSwitcher.svelte`
- `dashboard/src/lib/components/TokenGate.svelte`
- `dashboard/src/lib/components/OnboardingWizard.svelte`

Audit each call site of `upsertProfile`/`removeProfile`/`setActiveProfile`
to handle the new async semantics. Currently sync — wrap with
`await` and surface errors via the existing toast helper.

The first-run "no endpoint configured" flow keeps working because the
loopback POST happens against the same daemon that's serving the SPA.

**Commit:** `feat(dashboard): handle async profile mutations in UI`

### 6. End-to-end check

Manual:
- Add a profile in TUI (`agentum profiles add foo https://foo:8822`),
  refresh dashboard, profile appears.
- Add a profile in dashboard (EndpointSwitcher → +), TUI shows it on
  next `agentum profiles list`.
- Delete from one side, verify other side sees it gone after refresh.
- Existing localStorage profiles get migrated on first dashboard load.

Automated:
- `cargo test -p agentum-server profiles` for the new route tests.
- `npm run check --prefix dashboard` passes.

**Commit:** none for the verify step itself.

## Risks

| Risk | Mitigation |
|------|------------|
| Atomicity: TUI and dashboard write concurrently | Filesystem write is whole-file rewrite; last-writer-wins is acceptable for human-paced edits. Not a 100Hz API. |
| `Profiles` move breaks downstream tests | Audit `cargo test --workspace` after step 1 before moving on. `crates/agentum-store`'s pre-existing breakage (see CLAUDE.md) is the known noise floor. |
| Dashboard served by remote daemon shows "wrong" profiles | Documented as intentional in non-goals. EndpointSwitcher label should make the source clear (`profiles from @vps`). |
| First-load API failure leaves dashboard with no profiles | Fall back to localStorage cache + show a "couldn't reach daemon" banner. Existing behaviour when daemon is down. |
| `agentum-core` accumulating filesystem-touching code | Acceptable for now (`paths` already lives in `agentum-store`). If it grows, split into `agentum-config` later. |

## Rebuild rhythm reminder

Per CLAUDE.md: after any change under `dashboard/src/`, must run
`npm run build --prefix dashboard && cargo build --release` to re-embed
the SPA into the daemon binary. CI does this automatically; local
testing requires it.

## Future work (out of scope)

- WebSocket broadcast on profile mutation so multi-client dashboards
  stay live without refresh. Reuse `events.rs` bus.
- `agentum profiles export` / `import` for sharing profile sets across
  machines without retyping.
- Sync labels server-side (currently localStorage-only) once we have a
  reason to need cross-browser label consistency.
