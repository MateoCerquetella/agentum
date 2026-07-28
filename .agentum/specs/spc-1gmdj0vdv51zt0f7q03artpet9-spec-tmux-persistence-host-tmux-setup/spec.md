---
schema: 1
id: SPC-1GMDJ0VDV51ZT0F7Q03ARTPET9
revision: 1
title: Spec: tmux persistence + host tmux setup
source: legacy-import:ai/specs/005-tmux-persistence-and-host-setup/spec.md@sha256:db217eb755ccd48e5a985e7600d885eacc4c47796ac3075d9d18478159166550
---

# Spec: tmux persistence + host tmux setup

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec: tmux persistence + host tmux setup
>
> ## Goal
>
> Let a user opt a new terminal/agent into a persistent tmux session that
> auto-reattaches when agentum reopens, and set tmux up on an SSH host in one click.
>
> ---
>
> ## User Value
>
> A developer keeps an agent running after closing agentum and finds it
> auto-reconnected on reopen — local or over SSH — without touching the terminal.
>
> ---
>
> ## Requirements
>
> - **(C) Opt-in persist toggle** — the New Terminal and New Agent flows expose a
>   "Run in tmux (persist)" toggle. On → the session runs in a named tmux session
>   on its host (local or SSH). Off → ephemeral (not tmux-backed). Default: **on**
>   (matches the product vision + current behaviour; off is the explicit opt-out).
> - **(C) Silent auto-reattach** — on relaunch, any pane whose tmux session is
>   still alive on its host reconnects automatically and shows the agent's current
>   state, with no user action and no duplicate/fresh session spawned.
> - **(A) One-click tmux install** — when a host's readiness probe reports tmux
>   missing, the host UI offers an "Install tmux" action that installs it via the
>   existing bootstrap path and flips `tmuxInstalled` true on the next probe.
> - **(A) Sane-default tmux.conf** — the install action also ensures a sensible
>   tmux config on the host (mouse on, large scrollback, sane prefix), without
>   destroying a pre-existing user `~/.tmux.conf` (back up or no-op if present).
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] New Terminal and New Agent show a "Run in tmux (persist)" toggle; with it
>       **off**, the created session is not tmux-backed.
> - [ ] A session created with the toggle **on** runs inside a named tmux session
>       on its host (confirmable via `tmux ls` locally / `ssh … tmux ls` remotely).
> - [ ] After quitting and relaunching agentum, a pane whose tmux session is still
>       alive auto-reattaches and displays the agent's current output without any
>       user action (no second session is spawned).
> - [ ] An ephemeral (toggle-off) terminal does **not** survive quitting agentum.
> - [ ] When a host lacks tmux, the host UI shows an "Install tmux" action that
>       installs it; the host's readiness then reports `tmuxInstalled = true`.
> - [ ] The install action results in a tmux config with mouse mode on and
>       scrollback ≥ 10000 lines, and does not overwrite an existing user
>       `~/.tmux.conf` (it backs it up or skips).
>
> ---
>
> ## Dependencies
>
> - `002-sidebar-host-grouping`, `003-sidebar-host-metadata` — the host surface
>   (header/readiness) where "Install tmux" and the tmux state live.
> - The remote git/worktree/agent layer (`host_runtime`, host-aware execution) and
>   the existing `POST /api/hosts/{id}/bootstrap` (already installs tmux/git).
> - Existing per-session tmux backing (`shouldUseServerTerminals` → server session
>   → `agentum_tmux` new-session) and the host readiness `tmuxInstalled` signal.
>
> ---
>
> ## Risks
>
> - **Reattach mis-mapping** — reconnecting a reopened pane to the correct live
>   tmux session (by tmux target) is the crux; attaching to the wrong session or
>   spawning a duplicate would corrupt state. Needs a stable pane→tmux-target map.
> - **SSH auth on reattach** — reattaching over SSH must reuse the host's stored
>   auth (key/agent/password); a password host must not block on a prompt.
> - **Clobbering tmux.conf** — writing a default config could destroy a user's
>   existing host config; must back up or no-op when one exists.
> - **Default-on persistence** — defaulting the toggle on changes nothing today
>   (sessions are already tmux-backed) but the *ephemeral* path is new and must
>   truly tear down on close. (PM to confirm default direction.)
>
> ---
>
> ## Notes
>
> **Out of scope (future specs):**
> - **(B) Open a project into its existing tmux from the sidebar** → later spec
>   (008; 006 = host-first-new-workspace, 007 reserved for a possible A split).
> - Editing arbitrary tmux.conf from agentum (we only apply sane defaults).
> - Windows hosts.
>
> **Decisions (from spec Q&A):**
> - First slice = C + A together (cohesive: A enables C on SSH hosts). B deferred.
> - Reattach is **silent/automatic**, not a prompt.
> - tmux backing is a **toggle**, default on (persist); off = ephemeral.
> - tmux config = **install + sane defaults**, not a full editor.
>
> **PM note (2026-06-06):** kept A+C in one spec per the user's explicit request;
> they form one cohesive journey and A is a small enabler. If the Architect finds
> the combined design too large for one slice, split **A → spec 007** (host tmux
> install + sane-default tmux.conf; 006 is host-first-new-workspace) and keep 005
> as C-only (persistence). C is independently shippable for local hosts (local
> tmux already present); A only gates SSH-host persistence.
