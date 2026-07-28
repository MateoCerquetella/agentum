---
schema: 1
id: SPC-1H25GMET2A48MJKH7KR7159WKY
revision: 1
title: Spec: Hosts-first sidebar — grouping + host header
source: legacy-import:ai/specs/002-sidebar-host-grouping/spec.md@sha256:d0bce02990ef75a28622a04a2cfe25090fb1c2fd50b6efae5f8777c4c7852f9e
---

# Spec: Hosts-first sidebar — grouping + host header

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

> # Spec: Hosts-first sidebar — grouping + host header
>
> ## Goal
>
> Make the desktop sidebar's primary layout **HOST → PROJECT (repo) → WORKTREE**
> instead of today's flat worktree list. Each host gets a header row — local vs
> SSH icon, name, a **reachability dot**, and a **live-session count badge** — that
> collapses/expands and persists. This is the structural foundation the later
> enrichment specs ([[003-sidebar-host-metadata]], [[004-sidebar-session-activity]])
> build on, and it ships the count badge on its own.
>
> ---
>
> ## User Value
>
> **In one line:** glance at the sidebar and see which hosts have running,
> tmux-backed sessions — so "close the IDE, agents keep working on the host" is
> *visible*, not a leap of faith.
>
> - **Who:** the user running agents across a local machine + one or more SSH
>   hosts (e.g. Omarchy). They asked directly: *"is there a way to know if my
>   project/host is tmux-attached? … close agentum and it should keep working."*
> - **Why now:** remote SSH projects now fully work (worktrees/git/agents over
>   SSH), but the sidebar has **no host level** — a remote project looks identical
>   to a local one, and there's no at-a-glance signal of live sessions per host.
> - **Cost of doing nothing:** no way to see, per host, that work is still running;
>   the desktop diverges from the TUI's host→project→session tree.
>
> ---
>
> ## Requirements
>
> - **`repoHostKey(repo)`** helper — analogue of the TUI's `host_group_key()`
>   (`crates/agentum-cli/src/commands/terminal/app.rs`). A repo with a
>   `connectionId`/`hostId` buckets under that host; a local repo (neither)
>   buckets under a synthetic `local` host key. Stable + pure.
> - **`host-header` row type** added to the row discriminator in
>   `components/sidebar/worktree-list-groups.ts` (today:
>   `'header' | 'item' | 'imported-worktrees-card'`). Rows emit in
>   **host → repo → worktree** order; host-first is the **default** layout.
> - **Existing repo-group header + worktree leaf rows are reused unchanged**
>   (this spec only adds the host super-level above them).
> - **`HostGroupHeader.tsx`** (new) renders one host row: `Monitor` (local) /
>   `Server` (ssh) lucide icon, host name/label, reachability dot, session-count
>   badge, expand/collapse chevron.
> - **Reachability dot** — `reachable` (green) / `connecting` (amber) /
>   `down`·`unknown` (gray). Source: local host is always reachable; SSH hosts map
>   from `sshConnectionStates` (`connected`→reachable, `connecting`/`deploying-*`→
>   connecting, else down/unknown).
> - **Session-count badge** — `● N` pill = number of worktrees (sessions) grouped
>   under that host. Hidden or `0`-styled when none.
> - **Expand/collapse per host**, **persisted**; default = local host expanded,
>   others collapsed. Collapsing a host hides its repo/worktree rows.
> - **Thin `slices/hosts.ts`** holding only what isn't derivable:
>   `hostStatusById` (reachability) and `hostExpanded` (persisted). OS/arch is
>   **out of scope here** (spec 003).
> - Keep the existing `groupBy` modes (`repo` / `workspace-status` / `pr-status`)
>   available; host-first becomes the default. Deleting the old modes is a
>   follow-up, not part of this spec.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] **Hierarchy** — with a mix of local and SSH repos, the sidebar **renders**
>       host headers, and under each host its repos and their worktrees, in
>       host→repo→worktree order; host-first is the default on a fresh load.
> - [ ] **Bucketing** — `repoHostKey()` **buckets** a repo with a `hostId`/
>       `connectionId` under its host and a local repo under the synthetic `local`
>       host (unit-tested: local-only, mixed, multiple repos per host, empty host).
> - [ ] **Count badge** — each host header **shows** `● N` equal to the count of
>       worktrees grouped under it (unit-tested on the builder output).
> - [ ] **Reachability dot** — the dot **reflects** `hostStatusById`: green when the
>       host's SSH state is `connected` (or it's local), amber while connecting,
>       gray otherwise.
> - [ ] **Collapse** — toggling a host header **hides/shows** its rows and the
>       state **persists** across reloads; local host defaults expanded.
> - [ ] **Icon** — `Monitor` for local, `Server` for SSH (render test).
> - [ ] **No regression** — switching back to a legacy `groupBy` mode still works;
>       virtualization/scroll behavior is preserved.
>
> ---
>
> ## Dependencies
>
> - Repo `hostId` is populated (the remote-git layer + legacy backfill already
>   ship this — `routes/repos.rs`, `fetchRepos` backfill).
> - Existing state consumed unchanged: `repos` (`slices/repos.ts`),
>   `worktreesByRepo` (`slices/worktrees.ts`), `sshConnectionStates` /
>   `sshTargetLabels` (`slices/ssh.ts`), the virtualized `WorktreeList`.
>
> ---
>
> ## Risks
>
> - **Regressing the daily-driver sidebar.** It's virtualized with pinned/imported/
>   lineage edge cases — host rows must thread through without breaking row keys or
>   scroll. Mitigation: builder unit tests + keep legacy modes intact.
> - **Row-key collisions.** Host headers add a new key namespace; must not clash
>   with `repo:`/`item:` keys (prefix `host:`).
> - **Persisted-collapse migration.** New `hostExpanded` store key — default sane
>   (local expanded) so existing users aren't met with everything collapsed.
> - **Count semantics.** "Sessions" vs "worktrees" per host — v1 counts worktrees
>   grouped under the host (matches the mockup `● N`); a true live-tmux count can
>   refine later.
>
> ---
>
> ## Notes
>
> **Out of scope (later specs):** host OS/arch metadata line (003); ctx% chip,
> unwired `PRIMARY` slot, and the active-session card (004); deleting the old
> `groupBy` modes.
>
> **Reference:** the TUI tree `Tree::build_with_profiles()` + `host_group_key()` in
> `crates/agentum-cli/src/commands/terminal/app.rs` is the shape to mirror. Design:
> `docs/superpowers/specs/2026-06-05-desktop-hosts-sidebar-design.md` (§3 Hybrid,
> §4.1/4.2). Styling stays Tailwind + `--sidebar*` vars + Radix Accordion +
> lucide-react (no new design system).
