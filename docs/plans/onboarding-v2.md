# Onboarding v2 — one-question wizard

Status: planned (target: next minor release after current)

## Why

The current `install.sh` asks 3 mode questions (Control Plane / Terminal
CLI / Both) plus a separate autostart prompt plus an auth setup prompt.
It conflates two things that should be separate:

1. **What's installed** — always the one binary. There's no real choice.
2. **What this machine does** — runs a daemon, connects to a remote
   daemon, or both. This is the only choice that matters to the user.

First-time users get walked through "host vs hosted / server vs client"
vocabulary they haven't earned yet. The dashboard URL we hand them
(`https://127.0.0.1:8822`) doesn't work from their phone on the same
LAN, which is one of the main reasons someone runs a control plane.

## Mental model we're teaching

> One binary. One question: **will this machine run agents, or just
> connect to a remote one?**

Everything else is inferred or deferred. Multi-server / bidirectional
control isn't taught in the wizard — it's a one-line tip at the end.

## Wizard flow

```
█ agentum installer
  platform · version · install dir

  ▸ Will this machine run agents?
    [1] Yes — run agents here (recommended)
    [2] No — just connect to a remote agentum
    Choice [1-2] (1): _
```

### Path A — "run agents here"

1. Download + install binary (always).
2. tmux check (existing).
3. `agentum auth setup` prompt (existing, unchanged).
4. Autostart prompt (existing — background / systemd unit / skip).
5. **NEW**: detect LAN IP, register `local` profile pointing at it,
   mark as default. So `agentum terminal` Just Works and the
   dashboard URL we hand them is reachable from other devices.
6. Final summary:
   ```
   ✓ agentum is running on this machine
     Dashboard:  https://192.168.1.42:8822
     TUI:        agentum terminal

     Tip: have agentum on another machine (VPS, work laptop)?
          Run `agentum profiles add` to switch between them.
   ```

### Path B — "just connect to a remote"

1. Download + install binary.
2. tmux check (existing).
3. **Skip** auth setup (nothing to auth against locally).
4. **Skip** autostart.
5. Prompt for remote URL: `Remote agentum URL: _`
   - Empty → skip, hint at `agentum profiles add` later.
   - Filled → shell to `agentum profiles add remote <url> --set-default`.
6. Final summary:
   ```
   ✓ agentum installed (client mode)
     TUI:        agentum terminal

     Tip: this machine can also run agents — `agentum serve` anytime.
   ```

## LAN IP detection

POSIX-portable fallback chain:

- **macOS**: `ipconfig getifaddr en0`, then `en1`, …, then default route.
- **Linux**: `hostname -I | awk '{print $1}'`, fallback to
  `ip route get 1.1.1.1 | awk '{print $7; exit}'`.
- **Fallback**: `127.0.0.1` with a warning that the dashboard will only
  be reachable from this machine.

Implement as a single `detect_lan_ip()` shell function. Returns
empty on failure; the wizard falls back to `127.0.0.1` and prints a
note.

## Out of scope (defer)

- Detecting whether the user is on a public VPS vs a laptop and
  changing defaults. The autostart prompt already covers this.
- Auto-bootstrapping the remote profile's TLS fingerprint. The
  existing `agentum profiles add` accepts `--fingerprint`, but
  fetching it requires hitting the remote's `/api/cert/fingerprint`,
  which is its own loop (timeouts, TOFU UX). Save for v3.
- Dashboard parity — the web onboarding stays unchanged for now.
  The first-run flow there is already lighter; we revisit if the
  CLI changes here expose mismatches.

## Files touched

- `scripts/install.sh` — the bulk of the work.
- `CHANGELOG.md` — note the wizard change.
- No Rust changes expected; `agentum profiles add` already does
  everything the script needs.

## Release plan

Ship as a patch release on top of whatever's current. The change is
install-time only — existing installs are not affected on update
(the wizard short-circuits when `IS_UPDATE=true`).
