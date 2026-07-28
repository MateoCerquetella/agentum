---
schema: 1
id: SPC-0HCPBYBS54M5SF6MWRRN1FMDHP
revision: 1
title: Spec: Hosts-first sidebar — host OS/arch metadata line
source: legacy-import:ai/specs/003-sidebar-host-metadata/spec.md@sha256:a9732a89b7b5b8c4a5f4d66144148e596dfb871ae08d48b206b3644188646373
---

# Spec: Hosts-first sidebar — host OS/arch metadata line

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

> # Spec: Hosts-first sidebar — host OS/arch metadata line
>
> ## Goal
>
> Add the **OS/arch metadata line** under each host header — e.g.
> `localhost · macOS 15 · M3 Max` (local) and `ssh forge.lan · Linux 6.9 · x86_64`
> (SSH) — so a host is identifiable at a glance, not just a name. Extends the
> `hosts` slice and header from [[002-sidebar-host-grouping]].
>
> ---
>
> ## User Value
>
> **In one line:** know *what* each host is (transport + OS + arch) without leaving
> the sidebar — useful when juggling a local Mac and one or more Linux SSH boxes.
>
> - **Who:** the multi-host user (local + SSH) who needs to tell hosts apart and
>   confirm a remote box is the expected machine.
> - **Why now:** once hosts are first-class rows (002), the bare name is thin; the
>   mockup calls for the OS/arch line as the second host enrichment.
> - **Cost of doing nothing:** hosts are distinguishable only by name; no
>   confirmation of the remote OS/arch the agents actually run on.
>
> ---
>
> ## Requirements
>
> - Extend `slices/hosts.ts` with `hostMetaById`:
>   `{ kind: 'local' | 'ssh'; label: string; os?: string; arch?: string }`,
>   plus a `hydrateHosts()` action.
> - **OS/arch sources:**
>   - **SSH host:** `uname` from the existing `POST /api/hosts/{id}/test`
>     (`{ok, tmux, git, uname}`) or `GET /api/hosts/{id}/readiness`
>     (`system.uname`). Parse `uname -sr`/`-m` into `os` + `arch`.
>   - **Local host:** `HostSystemInfo` (`system.uname`) from the embedded server's
>     readiness for the local host id, **if** reachable from the UI. (See Open
>     question — if not, add a tiny read-only endpoint/native call.)
> - **Fetched lazily** on sidebar mount and refreshed on host/SSH state-change
>   events; **cached** in the slice so we don't probe per render.
> - `HostGroupHeader` renders the metadata line under the name (muted), in the
>   format `<transport> · <os> · <arch>`; gracefully omits unknown parts
>   (`localhost · macOS 15` if arch missing; just the transport if both unknown).
> - Never block the header render on the probe — show the name + dot immediately,
>   fill the OS/arch line when it resolves.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] **SSH metadata** — an SSH host header **shows** `ssh <label> · <os> · <arch>`
>       derived from the host's `uname` (probe), with the transport prefix.
> - [ ] **Local metadata** — the local host header **shows**
>       `localhost · <os>[ · <arch>]` from `HostSystemInfo`.
> - [ ] **Lazy + cached** — metadata is fetched on mount / host-event, not per
>       render; a second render does not re-probe (cached in the slice).
> - [ ] **Graceful unknowns** — missing `os`/`arch` **degrade**: render only the
>       known parts; never `undefined`/blank fragments; never block the row.
> - [ ] **Parse** — `parse uname` **maps** representative `uname` strings
>       (Darwin/macOS, Linux x86_64/arm64) to `{os, arch}` (unit-tested).
>
> ---
>
> ## Dependencies
>
> - [[002-sidebar-host-grouping]] — host headers + the `hosts` slice must exist.
> - `POST /api/hosts/{id}/test` / `GET /api/hosts/{id}/readiness` (already return
>   `uname` / `system.uname`).
>
> ---
>
> ## Risks
>
> - **Local OS/arch reachability (open question).** If `HostSystemInfo` for the
>   local host isn't exposed to the UI today, this needs a tiny read-only
>   endpoint/native command — small but it's the one possible plumbing add.
> - **Probe latency / rate.** SSH `uname` probes cost a round trip; must be lazy +
>   cached + event-driven, not polled per render, to avoid SSH chatter.
> - **`uname` parsing variance.** Distros/versions format differently; parser must
>   degrade to "unknown" rather than show garbage.
>
> ---
>
> ## Notes
>
> **Out of scope:** reachability dot + count badge (002); ctx%/PRIMARY/active card
> (004). **Open question carried from the design:** confirm local `HostSystemInfo`
> is UI-reachable without a new command; if not, add a minimal read-only one.
> Design ref: `docs/superpowers/specs/2026-06-05-desktop-hosts-sidebar-design.md`
> §4.2–4.3, §9.
