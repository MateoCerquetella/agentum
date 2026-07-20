---
tracker: https://github.com/MateoCerquetella/agentum/issues/395
---

# Spec 009 — SDD loop injects into every agent tool

- **Number:** 009
- **Status:** Superseded
- **Surface:** `crates/agentum-server` (harness injection path) + `crates/agentum-executor` (per-tool adapter signatures)
- **Author:** Mateo (direct ask — GitHub issue #395, drafted via `/sdd-spec`)
- **Date:** 2026-07-20

> Superseded on 2026-07-20 by the later, run-owned authoring decision in
> `.agentum-harness/specs/395-sdd-loop-is-not-injectin-in-other-agents/spec.md`.
> That decision resolved the ambiguous issue title as a live SDD status-strip
> request; this injection-path interpretation remains a possible follow-up and
> was not the implementation contract for issue #395.

## Problem

Starting a gated run with any agent tool other than Claude leaves the SDD role
loop dead: the role agent spawns, sits at its idle screen, and the prompt never
lands — the harness polls ~56 s for Claude-only footer strings, gives up, fires
the prompt blind, and the role gate later times out (up to 30 min), fails,
retries, and goes blocked. On the rare attempt the blind paste happens to hit
the input box (~1 in 10 per the report) the loop advances. Net effect: the SDD
loop — the product's core loop — only works with Claude, and with every other
agent it fails slowly and confusingly. (This repo's own run config for issue
#395 sets `agent_tool: "cline"`.)

## Goal

Every prompt the SDD loop injects (PM-role gate, architect, feature, retry, QA,
review) is delivered through per-tool readiness + trust handling sourced from
the adapter layer, so injection is confirmed-landed for every first-class agent
tool — or fails loudly within seconds — with Claude's sacred send sequence
byte-identical.

## Users / personas

- **Mateo, solo dogfooder** — starts a gated run from an issue with the agent
  set to cline/codex/gemini, opens the role agent's pane, and watches it sit
  empty while the run burns settle timeouts and retries. Filed it as "SDD-loop
  is not injecting in other agents … it should check the status of the SDD".
- Secondary: any user who picks a non-default agent for gated runs and assumes
  the role loop works the same as with Claude.

## Acceptance criteria

1. Readiness dispatch **returns** per-tool results: given a `Session.tool`, the
   harness readiness wait matches THAT tool's idle-input signature (sourced
   from the adapter layer), and a unit test proves a Claude footer does NOT
   satisfy a non-Claude tool's readiness, and vice versa.
2. Trust/onboarding screens that swallow pastes (codex/cursor/copilot class)
   are accepted or pre-written per the adapter BEFORE any prompt bytes are sent
   — observable: no `send_bytes` fires while the tool's trust screen is visible
   in the pane (pane-fixture test).
3. `inject_prompt` **returns** `Ok(true)` (confirmed, not blind) for a
   non-Claude first-class tool whose idle signature appeared, and every
   dispatch **emits** a landed(confirmed)/fired-blind log line naming the tool
   — so "check the status of the SDD" is answerable from the run's event log
   without opening the pane (extends the spec 008 #14a loud line).
4. Claude regression guard: the Claude arm's poll strings, trust-accept, and
   two-step send sequence (`send_bytes` → `SUBMIT_DELAY` → bare Enter) remain
   byte-identical, and the existing `#[ignore]` live tests
   (`harness_start_work_live{,_roles}.rs`) still pass.
5. Live proof on the reported path: a gated run with `agent_tool: "cline"` (the
   failing config from issue #395) **renders** the PM-role-gate prompt's marker
   text in the role agent's pane — a new `#[ignore]` live test mirroring the
   MARKER assertion in `tests/support_start_work/mod.rs`, human-gated like AC 4.

## Scope & non-goals (YAGNI)

- **In:** server-side per-tool readiness signatures + trust handling inside the
  harness injection path; per-dispatch landed/blind visibility; Claude parity;
  one live test on the reported non-Claude path.
- **Out:** argv/flag/env prompt delivery at spawn (the desktop draft-launch
  `promptInjectionMode` domain — the harness keeps paste-into-pane delivery);
  any change to Claude's send sequence; SSH/remote-pane readiness (stays
  fixed-delay blind fallback); any new "SDD toolbar" UI surface (see open
  questions); role-gate retry/backoff redesign.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `inject_prompt` (`crates/agentum-server/src/harness/drive.rs:1037`) — the ONE
  injection path; all four SDD call sites already funnel through it (feature
  :162, retry :361, QA :611, role gate :750). The fix lands here, not per call
  site.
- `await_repl_ready` (`drive.rs:967`) — the poll / trust-accept / bounded
  fallback loop to dispatch per tool; keeps the ~56 s ceiling + loud blind
  fallback shape.
- `ToolAdapter` + `adapter_for` (`crates/agentum-executor/src/adapters.rs`) —
  per-agent signatures (busy/awaiting-input/crash) already consumed by the
  watchdog; per-tool delivery knowledge already lives here in precedent (Codex
  MCP argv, :170). The natural home for idle/trust signatures (principle 5:
  adapter, not special-case).
- Desktop per-agent delivery knowledge to port/consult:
  `crates/agentum-desktop/ui/src/shared/tui-agent-config.ts`
  (`promptInjectionMode` :16, `draftPasteReadySignal` :49, `preflightTrust`
  :43), `agent-paste-draft.ts`, `agent-trust-presets.ts`.
- Loud-status builders `repl_not_ready_message` (`drive.rs:1124`) /
  `settle_timeout_message` (:1113); run-status plumbing `SpecPhase`
  (`harness/types.rs:284`) + `set_phase`/`phase()` (`harness.rs:624/:654`) +
  durable `decisions.md`.
- Live-test scaffold `crates/agentum-server/tests/support_start_work/mod.rs`
  (MARKER-in-pane assertion proves a prompt landed).

### Build new

- Per-tool idle-input + trust-screen signatures in the adapter layer
  (server-side), with `await_repl_ready` dispatching on `Session.tool`.
- Trust handling for the paste-swallowing tools before first injection.
- Per-dispatch "prompt landed (confirmed) / fired blind, tool=<x>" line at
  every `inject_prompt` call site.
- One `#[ignore]` live test: non-Claude role-gate delivery (MARKER pattern).

## Risks & invariants

- **Sacred mechanics (spec 008 D5).** Claude's `await_repl_ready` poll/trust
  strings and the two-step `inject_prompt` send sequence change NOT AT ALL —
  per-tool dispatch must be additive (Claude arm = existing code verbatim).
  Merge gate: both Claude live tests green (AC 4).
- **One launch path.** All changes stay inside the harness injection path; no
  client-side prompt pushing, no per-call-site special-casing (principles 1, 5).
- **False-ready risk.** A loose per-tool signature could confirm readiness
  while the agent is still booting → paste lost → same bug, quieter. Signatures
  must come from observed panes of the real tools; the bounded blind-fire +
  loud log stays the fallback, never silence.
- **Wait-time risk.** Per-tool polling keeps the existing ~56 s ceiling so a
  hung boot can't stall the role loop longer than today.

## Harness wiring (the gate)

- **feature_list.json entries:** F1 per-tool readiness dispatch + adapter
  signatures (AC 1, 3); F2 trust-screen handling for paste-swallowing tools
  (AC 2); F3 Claude parity guard + non-Claude live test (AC 4, 5).
- **`verify.sh` asserts:** `cargo test -p agentum-executor --lib` (signature
  units) + `cargo test -p agentum-server --lib` (dispatch/fixture tests) green;
  fmt + clippy clean.
- **`qa.sh` asserts:** n/a for this server-only path — the two `#[ignore]` live
  tests (Claude + cline) are the human pre-release gate, same contract as spec
  008's D5 live tests.

## Open questions

1. **"1/10" decode.** Read as "the blind paste lands ~1 in 10 attempts"
   (intermittent delivery). Alternative reading: role prompts should also carry
   SDD-status context (read `ai/STATE.md` / `/sdd-status`) so agents actively
   "check the status of the SDD". Confirm at PM.
2. **"(SDD toolbar…)".** No `SddToolbar` component exists in the codebase;
   nearest surfaces are the composer's "Start gated run" armed copy
   (`NewWorkspaceComposerCard.tsx`) and Settings → "SDD role loop on gated
   runs" (`IntegrationsPane.tsx`). Which UI element did the reporter mean, if
   any?
3. **Tool coverage for the gate:** full `FIRST_CLASS` registry, or the dogfood
   set first? Spec defaults to: signatures for all first-class tools, live test
   on cline (the reported config).
4. **SSH/remote panes** stay on the fixed-delay blind fallback for this slice —
   acceptable?
