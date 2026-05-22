---
slug: server-switcher-bugs
status: root_cause_found
trigger: |
  Two bugs in the TUI server switcher reported after the v0.8.0 release.
  Both are visual — no data loss.

  Bug 1 — Stale server-version chip:
    `agentum --version` shows 0.8.0 on the user's machine but the
    per-server version chip in the sidebar still reads `v0.7.68`.

  Bug 2 — Persistent synthetic "local" server entry:
    The SERVERS section in the TUI sidebar always shows a hardcoded
    loopback row (label "MY MACHINE (omarchy)") even when the user
    has explicit profiles registered (e.g. "mateos-macbook-pro").
created: 2026-05-20T14:50:00Z
updated: 2026-05-20T15:05:00Z
goal: find_root_cause_only
---

## Resolution

### Bug 1 — Stale server-version chip

**Verdict: NOT A CODE BUG. Expected behavior.**

The version chip already auto-refreshes correctly. Trace:

- `crates/agentum/src/commands/terminal/app.rs:2321-2326` — at run-loop
  startup the active client's `Client::health()` is fetched once so the
  first frame can paint a real version chip instead of `v?`.
- `crates/agentum/src/commands/terminal/app.rs:2680-2691` — on every
  `tick` interval, every live client (loopback `""` + every named
  profile) is re-probed in parallel via
  `futures_util::future::join(c.list_sessions(), c.health())`. The
  comment at lines 2674-2679 explicitly states the rationale:
  *"Probe both endpoints per client so the periodic tick keeps the
  sidebar version chip honest after a remote daemon is upgraded +
  restarted. Without the health() leg, entry.version stayed pinned to
  the value captured at boot."*
- `crates/agentum/src/commands/terminal/app.rs:2728-2732` —
  `entry.version = Some(h.version.clone())` on each successful probe.
- `crates/agentum/src/commands/terminal/ui.rs:479-487` —
  `server_version_chip()` renders the cached version. When
  `v != env!("CARGO_PKG_VERSION")` it paints in the warning color so
  drift is visible at a glance.

What the user is seeing: the *running daemon* at PID 631439 is genuinely
still v0.7.68. Earlier I installed v0.8.0 to `~/.local/bin/agentum`
with `install -m 755`, which atomically unlinks the old file and
creates a new one — the kernel keeps the original v0.7.68 binary's
inode mapped for the running process. The chip correctly reports what
the daemon is actually serving via `/api/health` (`"version":"0.7.68"`,
confirmed earlier in this session).

**Fix: no code change.** The user needs to restart their daemon:

```
pkill -f 'agentum serve'
agentum serve --host 0.0.0.0 &
```

The warning-colored chip is the system telling them they have version
drift — that's already the intended UX. We could surface this more
loudly (e.g. a one-time toast on first poll when local CLI > daemon
version), but that's a separate enhancement.

### Bug 2 — Persistent synthetic "local" server entry

**Verdict: real UX bug. Hardcoded synthetic row regardless of context.**

`crates/agentum/src/commands/terminal/ui.rs:540-613` always renders a
synthetic loopback row at position 0 of the SERVERS section,
unconditionally. `server_count = app.profiles.len() + 1` at line 544
bakes the `+1` into the count. The comment at lines 572-575 makes the
design intent explicit:

> The local loopback is keyed by "" in `app.clients` when the user
> launched without `--profile`. With a `--profile` launch there's no
> local entry — we render the row anyway so the sidebar shape doesn't
> shift around launch flags.

That UX choice ("never shift the shape") doesn't match the user's
mental model: they registered `mateos-macbook-pro` as their target
and don't expect a phantom "MY MACHINE (omarchy)" row sitting above
it. When the user is *also* running a local daemon (their case — pid
631439), the synthetic row IS the local daemon, but the label
"MY MACHINE" and lack of explanation makes it read as a placeholder.

The Ctrl-S overlay deliberately does NOT synthesize a loopback row —
it only iterates on-disk profiles. So the overlay and sidebar are
already inconsistent on this point.

**Proposed fix:** drop the synthetic loopback row from the sidebar
SERVERS section when at least one named profile is present whose URL
points at the local loopback. Detection: parse the URL's host with
the existing `url::Url` crate and check whether it resolves to
`127.0.0.1` / `::1` / `localhost`. If yes, the named profile is the
"real" representation of the local daemon and the synthetic row is
redundant.

Smaller alternative the user may prefer: hide the synthetic row
whenever any named profile exists at all (matches the user's literal
phrasing: *"why do I have always a local servers? even that i have
it here my mateos-macbook-pro"*). This avoids the URL-parsing edge
cases (custom ports, IP literals vs hostnames, tunnel rebinds).

Either way, the change is local to ui.rs:540-613 — the `+1` at line
544 becomes conditional, and the row block at 558-612 wraps in an
`if should_render_synthetic_loopback(&app) { ... }` guard. Cursor
math at line 564 (`servers_cursor == 0`) and at line 618
(`(i + 1) == servers_cursor`) shifts to match. Sidebar's `Servers`
section keystroke handlers at app.rs:4106-4119 (the `a`/`d` keys
when `tree_section == TreeSection::Servers`) and the
`servers_collapsed` count math need a matching adjustment so cursor
navigation stays consistent.

### Shared infrastructure?

No. Bug 1 is correct behavior the user misread; Bug 2 is a ui.rs-only
change. They share the same screen but the data paths and root causes
are independent.

## Current Focus

- hypothesis: confirmed — see Resolution above
- test: visited the runtime data flow (App::clients populated at
  startup app.rs:2327, refreshed at tick app.rs:2680-2732) and the
  rendering site (ui.rs:540-613)
- expecting: a fix to Bug 2 is the only code change needed; Bug 1
  resolves itself once the daemon is restarted onto v0.8.0
- next_action: surface findings to user; await go/no-go on the Bug 2
  fix variant (hide-when-any-loopback-profile vs hide-when-any-named-profile)

## Evidence

- timestamp: 2026-05-20T15:00Z — `Client::health()` polled in tick
  loop with explicit comment that it keeps the version chip current
  (app.rs:2674-2691)
- timestamp: 2026-05-20T15:01Z — chip color flips to warning on drift
  (ui.rs:485)
- timestamp: 2026-05-20T15:02Z — `/api/health` from the live daemon
  returned `{"version":"0.7.68",...}` earlier this session, matching
  the chip
- timestamp: 2026-05-20T15:03Z — synthetic loopback row rendered
  unconditionally at ui.rs:558-612, count hardcodes `+1` at line 544
- timestamp: 2026-05-20T15:04Z — Ctrl-S overlay does NOT render a
  synthetic loopback row (ui.rs `draw_profiles_overlay` iterates
  `state.entries` only — no synthesized row)

## Eliminated

- hypothesis: "version chip caches forever after first connect" —
  eliminated; the tick loop re-fetches `c.health()` every
  REFRESH_INTERVAL and overwrites `entry.version` on success
  (app.rs:2728-2732)
- hypothesis: "synthetic loopback row already self-suppresses on
  --profile launch" — eliminated; the row is rendered regardless of
  launch flags, per the code comment at ui.rs:572-575
