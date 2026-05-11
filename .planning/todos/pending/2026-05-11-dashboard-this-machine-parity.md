---
created: 2026-05-11T00:00:00Z
title: Dashboard parity for new-session "this machine" + sidebar Servers polish
area: ui
files:
  - dashboard/src/lib/components/NewSessionDialog.svelte
  - dashboard/src/lib/components/Sidebar.svelte
  - dashboard/src/lib/profiles.ts
  - crates/agentum/src/commands/terminal/ui.rs
  - crates/agentum/src/commands/terminal/app.rs
---

## Problem

The TUI just shipped three new-session / sidebar fixes; the dashboard
still lags behind:

1. The TUI's New Session overlay no longer renders its title twice
   (box border + inner head). The dashboard's `NewSessionDialog.svelte`
   should be audited for any visible duplicate "Spawn session" /
   "New session" header on the various entry points (sidebar `+`,
   topbar buttons, Sessions page, MobileNav fab).

2. The TUI sidebar's SERVERS section now always shows a "this
   machine" row at the top — a synthetic entry at cursor index 0
   that maps to the empty / loopback profile. It appears even when
   `app.profiles` is empty, and clicking Enter on it switches to
   the local loopback. The dashboard sidebar currently has no
   SERVERS section at all; profiles live only in the topbar
   `EndpointSwitcher`. Decision pending: add a SERVERS section to
   the dashboard's `Sidebar.svelte` (matching TUI) OR rework the
   `EndpointSwitcher` to also always list a "this machine" entry.

3. The TUI new-session form's Servers field now resolves through
   `app.clients` (the empty key points at the real local loopback
   when one is connected) and re-fetches the daemon's `$HOME` via
   `client.list_dir(None)` when the user Tab-cycles. The dashboard's
   `NewSessionDialog.svelte` has no Servers/profile field — sessions
   always spawn on whatever endpoint the topbar switcher is pointed
   at. Audit whether to add a Servers picker that mirrors the TUI's
   cycle behaviour, including refetching workdir via `api.listDir()`
   when the picker value changes.

## Solution

TBD — likely:

- `dashboard/src/lib/components/NewSessionDialog.svelte`: add an
  optional Servers field above Working directory; on change, call
  `api.listDir(undefined)` against the selected profile's base URL
  and pre-fill `workdir` with `listing.path` (mirror the TUI's Tab
  handler in `crates/agentum/src/commands/terminal/app.rs` —
  search for `NewSessionField::Profile` in `handle_new_session_key`).
- `dashboard/src/lib/components/Sidebar.svelte`: add a SERVERS
  section above the sessions list that lists "this machine" + all
  configured peer profiles, with the same dot-coloring scheme as
  the TUI (live/unreachable/login-needed). Reuse the existing
  `$profiles` store from `$lib/profiles.ts`.
- `dashboard/src/lib/profiles.ts`: consider exporting a helper
  like `profilesWithLocal()` that yields a stable `[{ id: '', label:
  'this machine', baseUrl: '' }, ...profiles]` list so the dialog
  and sidebar share one source.

Reference the TUI changes for the design contract:
- `crates/agentum/src/commands/terminal/ui.rs`
  (`draw_sidebar` SERVERS rendering, `draw_new_session_overlay`)
- `crates/agentum/src/commands/terminal/app.rs`
  (`handle_new_session_key` Profile Tab cycle, navigation handlers
  in `handle_tree_key` — `servers_cursor` now indexes
  `profiles.len() + 1` rows with row 0 = "this machine").
