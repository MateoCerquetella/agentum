# Spec: Host CDP-Chromium Browser + Bound Playwright MCP — SSH Host (009c-2)

> Child of **009c** (PM split, 2026-06-18). The **second** half: generalise 009c-1's exact
> "one browser" wiring to an **SSH host**, reusing 009a's tunnel + screencast. **Supersedes
> 009b's separate-host-MCP path** (the MCP now binds to agentum's displayed host browser
> instead of being a separate hidden one).

> **STATUS: BLOCKED** — hard-depends on **009a** (host substrate + live-view, currently in
> developer phase, code not started) **and 009c-1** (the wiring being generalised). Do NOT
> take to Architect until 009a code lands and 009c-1 is done.

## Goal

On an **SSH host**, agentum runs a **CDP-controllable Chromium-class browser** co-located
with the work, with a **Playwright MCP bound to that exact instance** over CDP. The user
watches it **live in agentum** (over 009a's `ssh -L` tunnel + screencast), the agent drives
the **same** host browser, and the agent's results **persist on the host** and **re-surface
in agentum on reopen**. The wiring is the **same shape** as 009c-1 — no local special-case drift.

---

## User Value

**In one line:** the same "drive my browser" unification as 009c-1, but for a browser that
lives **on the host** — it reaches the host app's `localhost`, survives Mac sleep / agentum
close, and the user can still watch the agent drive it live while awake.

---

## Requirements

- The CDP-Chromium browser **+ bound Playwright MCP** run **host-side** (per 009a's
  host-resident model), co-located with the work.
- **Live-view over 009a's tunnel/screencast** — inherit, do not re-litigate the embed-vs-
  screencast decision. CDP travels over the forward (`ssh -L`) tunnel on 009a's CDP port range.
- The wiring is the **same shape** as 009c-1 — verified by a **shared code path**, not a
  forked local/host implementation.
- **Agent results persist on the host** (screenshots/assertions) and **re-surface in agentum
  on reopen** — host-execution proof (carry 009b's evidence stance).
- **Host dependency handling:** detect missing host Chromium/Playwright and **offer to
  install**, else **fail with a stated reason** — no silent hang.
- **Supersede 009b:** the separate-host-browser-MCP requirement in 009b is replaced by
  binding the MCP to agentum's displayed host browser. Don't build the throwaway separate-MCP path.

---

## Acceptance Criteria

- [ ] On an SSH host, the bound CDP browser + Playwright MCP run host-side; an agent action is **visible in agentum's live view** of that host browser.
- [ ] The wiring is the **same shape** as 009c-1 (no special-case local-path drift — verified by shared code path).
- [ ] Agent results **persist on the host** and **re-surface in agentum on reopen** (host-execution proof).
- [ ] Host missing Chromium/Playwright → **offer to install**, else **stated-reason failure**.
- [ ] CDP travels over 009a's `ssh -L` forward tunnel (no new bespoke transport).

---

## Dependencies

- **009a** (HARD) — host-resident browser substrate + live-view (CDP/screencast + `ssh -L`
  forward tunnel). Currently unbuilt (developer phase). 009c-2 cannot pass Architect until this lands.
- **009c-1** (HARD) — the local wiring this generalises. Build 009c-1 first.
- **009b** (extends / supersedes) — host-side MCP provisioning; bind to the displayed browser
  instead of a separate hidden one. Mark 009b's separate-MCP requirement superseded.
- **`mcp_provision.rs` / `sessions.rs` remote path** — wire the CDP endpoint + bound MCP into
  the **remote** agent launch (today: remote wires agentum MCP only).

---

## Risks

- **Local vs host divergence** — keep the wiring one shape; resist a special-case local path
  that drifts from the host path (this is an explicit acceptance criterion).
- **CDP over the tunnel** — latency / lifecycle of CDP across `ssh -L`; reuse 009a's port
  range and tunnel mgmt rather than inventing transport.
- **Agent reports green without driving** — persisted host screenshots/results are the hard evidence.
- **009a not built** — this spec is blocked on it; surfaced explicitly above.

---

## Out of Scope

- The local case (→ 009c-1, the dependency).
- The verification *loop* / orchestration (008b); issue-posting (008a/008b); the harness (010).
