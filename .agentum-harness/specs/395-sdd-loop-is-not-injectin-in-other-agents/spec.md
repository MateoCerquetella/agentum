# Spec 395 — Live SDD status strip on a gated run's agent sessions

> Source: GitHub issue https://github.com/MateoCerquetella/agentum/issues/395
> (title only, typos verbatim). PM decoding: "SDD toolbar is not injecting in
> agents" = no SDD status is visible on the agent sessions a gated run spawns;
> "it should be 1/10" = show feature progress as current/total (e.g. "1/10");
> "it should check the status of the sdd" = the strip reflects the live run status.

## Goal

When a gated SDD run is in flight, the user opens any agent session the run
spawned and sees a live status strip showing the run's current SDD phase and
feature progress n/N.

## Problem

A gated run spawns several agents (PM, architect, feature, QA, reviewer), but
none of their session views say anything about the run: you cannot tell which
SDD phase the loop is in or which feature of the backlog is being built without
leaving the session and interrogating the status API yourself.

## Persona

Mateo, solo dogfooder, watching a gated run's agent terminals in the desktop
app the moment he asks "which phase is it in, and which feature is it on?"

## User value

At a glance: which SDD phase the run is in and which feature (of N) each agent
is building — without leaving the session view.

## Current state (grounded)

- Server tracks it all: `SpecPhase` + `HarnessEvent::PhaseChanged` +
  `phase`/`phase_attempts` in `crates/agentum-server/src/harness/types.rs`
  (lines 277, ~492, 525-530); gates run in `crates/agentum-server/src/harness/drive.rs`.
- Status is already exposed: `crates/agentum-server/src/routes/harness.rs`, typed
  client-side as `HarnessStatus` (`phase`, `phase_attempts`, `features`,
  `current_feature`) in `crates/agentum-desktop/ui/src/runtime/harness-client.ts:77-90`.
- Reuse before build: `listHarnesses()`/`getHarnessStatus()` and the
  `phase_changed` event type exist in `harness-client.ts` with **zero UI consumers**.
- Run-owned sessions are identifiable by name: `harness-<feature-id>-<id8>`,
  `harness-qa-<feature-id>-<id8>`, `harness-<role>-<id8>`
  (`crates/agentum-server/src/harness/drive.rs:429,522,668`).
- No component under `crates/agentum-desktop/ui/src/components/` renders SDD
  phase/progress today (`components/harness/` contains only `ChatPage.tsx`).

## Acceptance criteria

- [x] Opening any agent session spawned by a gated run renders an SDD status
  strip on that session view showing the run's current phase name
  (e.g. Authoring, Architecture, Decompose, Executing, Review, Done, Blocked).
- [x] During the Executing phase, the strip renders feature progress as "n/N"
  (current feature position out of total backlog features, e.g. "1/10").
- [x] When the run advances phase or changes current feature, the strip updates
  without the user reloading the app (driven by the existing harness
  status/event channel, not a hand-rolled poller).
- [x] The strip renders on every session of the run — feature agents, QA
  agents, and role-gate agents (PM/architect/reviewer) — not just one of them.
- [x] Sessions that do not belong to a gated run render no SDD strip.

## In scope

- One read-only status surface on run-owned agent session views in the desktop
  UI; wiring it to the existing harness status/events.

## Out of scope (non-goals)

- Changes to the SDD loop itself: gates, verdicts, retries, decompose, prompts.
- Interactive controls (approve/retry/skip/pause) on the strip.
- A standalone Harness dashboard page; CLI/tmux-native status bars; injecting
  text into the agent's REPL.
- Renaming sessions or changing how run agents are spawned.

## Notes

- No conflict: only spec in `.agentum-harness/specs/`; ai/specs 006 (role
  gates default-on, rich issues) and 008 (loop end-to-end) are Done and do not
  cover surfacing run status on agent sessions.
