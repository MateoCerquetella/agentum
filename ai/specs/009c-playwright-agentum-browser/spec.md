# Spec: Playwright MCP Bound to agentum's Browser — Local + Host (009c)

> **STATUS: SPLIT (PM gate, 2026-06-18).** Fails "fits one screen" — confirmed by the
> sdd-pm gate. Cut into two dependency-ordered children:
> - **009c-1** — CDP-Chromium browser + bind Playwright MCP **locally** (this machine).
>   No blocker; **build first**. → `ai/specs/009c-1-local-cdp-browser-bind/spec.md`
> - **009c-2** — same wiring on an **SSH host** (reuse 009a tunnel/screencast + persisted
>   results). Hard-depends on **009a** (unbuilt, in dev) + **009c-1**; supersedes 009b's
>   separate-host-MCP path. → `ai/specs/009c-2-host-cdp-browser-bind/spec.md`
>
> Ordering: `009a (in dev) ─ 009c-1 (start now) ─ 009c-2 (after 009a + 009c-1)`.
> This parent file stays as the umbrella; the children are the implementable units.
>
> ---
>
> Child of **009** (host-resident browser umbrella). Generalises **009b** (host-only
> agent-MCP) so the **browser the user watches in agentum IS the one agents drive
> via Playwright MCP** — and it works **locally too**, not only on SSH hosts.
> Supersedes the WKWebView-only control path for agent-driven scenarios.

## Goal

agentum's browser is a **Playwright/CDP-controllable browser** (Chromium-class), and a
**Playwright MCP is bound to that exact browser instance**. An agent — local or on an SSH
host — drives the **same browser the user sees**: it navigates/clicks/snapshots, the user
watches it live in agentum, and the agent's results persist and re-surface on reopen.

---

## User Value

**In one line:** "drive my browser" means *my* browser — the agent automates the same
window I'm watching (local or on a host), instead of a hidden, separate headless instance
I can't see, with no way to take over.

Today there are **two disconnected browsers**: the one the user sees (native **WKWebView**,
driven by the `agentum_browser` MCP via injected JS — no real CDP, no screenshots) and a
**separate headless Playwright** instance at `:8931` the agent drives blind. The user can't
watch the agent's browser, and the agent can't drive the user's. This unifies them.

---

## Requirements

- agentum's browser surface is backed by a **CDP-controllable Chromium-class engine**
  (e.g. ungoogled-chromium / system Chrome / Playwright-managed Chromium), **not** WKWebView,
  for agent-driven tabs. The user **watches it live** in agentum (embed or CDP screencast).
- A **Playwright MCP is bound to that browser** over CDP (`--cdp-endpoint`), so the agent and
  the user share **one** browser instance/context — not two.
- Works **locally** (browser + MCP on this machine) **and on a host** (browser + MCP on the
  SSH host, per 009a's host-resident model); the **wiring is the same shape** in both.
- **MCP-agnostic seam:** Playwright is the reference binding; don't hardcode it as the only
  possible browser MCP (carry the 009b agnostic requirement forward).
- **Dependency handling:** detect a missing Chromium/Playwright (local or host) and
  **offer to install**; else **fail with a stated reason** — never a silent hang.
- The existing `agentum_browser` MCP (open/navigate/grab/annotate over the WKWebView) stays
  as the lightweight path; this spec is the **CDP/Playwright path** for real automation +
  shared-with-user control.

---

## Acceptance Criteria

- [ ] agentum shows a browser tab backed by a **CDP-controllable** engine; the user can watch it live
- [ ] A **Playwright MCP** is bound to that exact browser over CDP — an agent's `browser_navigate`/`click`/`snapshot` act on the **tab the user is watching** (verified: user-visible change from an agent action)
- [ ] The **same wiring works locally and on an SSH host** (browser + MCP co-located with the work)
- [ ] Agent results (screenshots/assertions) **persist** and **re-surface in agentum on reopen** (host-run proof, per 009b)
- [ ] Missing Chromium/Playwright is **detected with an offer to install**, else **fails with a stated reason** — no silent hang
- [ ] The Playwright binding is **not hardcoded as the only option** (agnostic seam preserved)

---

## Dependencies

- **009a** — host-resident browser substrate + live-view (hard dependency; reuse its CDP/screencast + `ssh -L` tunnel).
- **009b** — host-side browser-MCP provisioning; **extend** it: instead of a *separate* host browser MCP, **bind the MCP to agentum's displayed browser** over CDP.
- **`mcp_provision.rs` / `sessions.rs` remote path** — wire the CDP endpoint + Playwright MCP into local *and* remote agent launches (today: local wires agentum MCP + `:8931` headless; remote wires agentum MCP only).
- **`playwright_mcp.rs`** — already launches `@playwright/mcp` on `:8931`; add a **`--cdp-endpoint`** binding mode so it attaches to agentum's browser instead of spawning its own hidden Chromium. (Verified locally this session: the server launches + listens; Playwright browser automation works on this machine.)
- Chromium-class engine embeddable/displayable in agentum (the WKWebView→Chromium shift is the heavy slice; connects to the earlier "use ungoogled-chromium" ask).

---

## Risks

- **WKWebView → CDP-Chromium is a large substrate change.** Mitigate: scope the CDP browser to *agent-driven / shared* tabs first; keep WKWebView + `agentum_browser` for the lightweight path. Don't rip out WKWebView wholesale.
- **Two-browsers→one is the whole point but also the hardest part** — sharing one CDP context between the user's view and the agent's MCP must not race (who owns navigation, focus, lifecycle).
- **Live-view of a CDP Chromium** (embed vs screencast) is the heaviest UI slice — inherit 009a's decision rather than re-litigate.
- **Local vs host divergence** — keep the wiring one shape; resist a special-case local path that drifts from the host path.
- **Agent reports green without driving** — persisted screenshots/results are the hard evidence (carry 009b's stance).
- **Over-abstraction** of "any browser MCP" — ship the Playwright CDP binding concretely, leave a seam, no plugin framework (YAGNI).

---

## Notes

**Relationship to this session's work:** the `agentum_browser` MCP (open/tabs/navigate/
grab/annotate over WKWebView, shipped on `staging`) is the *lightweight* control path and
stays. 009c is the *real-automation* path: a CDP-controllable browser the user watches and
Playwright drives — the same instance.

**Likely PM split** (probably fails "fits one screen" as one spec): **009c-1** = CDP-Chromium
browser substrate + bind Playwright MCP locally (browser the user sees == the one the agent
drives, on this machine); **009c-2** = same wiring on an SSH host (reuse 009a tunnel/screencast)
+ persisted results. Surface this to PM before implementation.

**Out of scope:** the verification *loop*/orchestration (008b); issue-posting (008a/008b);
the harness (010); replacing WKWebView for *non-agent* casual browsing.
