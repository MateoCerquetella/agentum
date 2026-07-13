# Spec 358b — Issue hover card shows the bound GitHub Project's Status

> Split out of spec `358-the-sdd-loop-sohuld-inject-itself-10-tim` at the PM
> gate 2026-07-13 (it was a rider added mid-run by Mateo; bundling it with the
> SDD-loop slice violated the one-slice rule). **Not yet PM-gated — run this
> spec through its own PM gate before building.** Full SDD document (contains
> both halves): `ai/specs/016-sdd-loop-checkin-and-issue-project-status/spec.md`.
> No dedicated GitHub issue yet; open one when this spec is picked up
> (the rider lives in-thread on issue #358).

## Problem

Mateo hovers an issue badge on a worktree card in the agentum desktop sidebar
to check where a ticket stands. The hover card shows only the open/closed
state and labels — the issue's actual GitHub Project column (Status: Todo /
In Progress / …) is invisible even when the repo has a Projects v2 binding,
so he has to open GitHub to see the board position.

## User value

The issue's Project board column is visible at a glance from the hover card —
no round-trip to GitHub.

## Goal (one slice)

The issue hover card shows the bound GitHub Project's Status option name for
the linked issue.

## Acceptance criteria

- [ ] When the repo has a Projects v2 binding (`routes/github_projects.rs::get_binding` on develop) and the hover card (`WorktreeCardMeta.tsx::WorktreeCardDetailsHover`) opens for a linked GitHub issue, it renders a Project-status chip with the issue's current Status option name, visually distinct from `IssueStateBadge` and `TrackerPhaseChip`.
- [ ] No binding / issue not on the project / fetch error → the card renders no chip (silent absence, never an error state).
- [ ] The status is fetched lazily on card open and cached per issue for the app session (GitHub rate limits); a second hover on the same issue triggers no new fetch.

## Non-goals (out of scope)

- Read-only: no write path to the Project (no drag/move from the hover card).
- No change to `IssueStateBadge` or `TrackerPhaseChip`.
- No server-side work unless the architect pins a server route over the
  recommended desktop Tauri command.

## Constraints / invariants

- Fetch only on card open + cache — never poll (GitHub rate limits).
- Silent absence on any error; a tracker hiccup must not break the hover card.
- **Branch from fresh `origin/develop`** — this worktree's checkout is
  v0.57.0-era; citations are against `origin/develop` @ `bee8dc2d`.

## Verification (the gate)

- `verify.sh`: desktop UI build (`bun run build`) + vitest for the fetch-cache
  model.
- `qa.sh`: hovering an issue badge on a Project-bound repo shows the Status
  chip; hovering on an unbound repo shows no chip.

## Open questions

1. Read path: desktop Tauri command beside `gh_get_project_view_table`
   (recommended — gh auth + the popover are desktop-side) vs. a server
   route — architect to pin.
