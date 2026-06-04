# Agent spinner for non-Claude agents — investigation & fix

**Status:** RESOLVED. The fix shipped is the title-flicker debounce (below). The
per-agent hook subsystem this doc originally planned was **abandoned** once the
real root cause was found — recorded here so the dead end isn't re-walked.

## Symptom

"When I use codex/opencode/whatever there's no spinning action like Claude
(orca) does." The sidebar working spinner appeared for Claude but not Codex.

## The wrong first hypothesis (abandoned)

Initial reading: Codex emits no working/idle signal in its OSC title, and
`CodexAdapter` injects no status hook (the per-agent hook installer lived in the
deleted Electron `src/main/agent-hooks/server.ts` and was never ported to
Tauri). Plan was to revive per-agent status hooks (Codex via `CODEX_HOME` +
managed `hooks.json`, etc.).

**Why it was wrong:** the desktop derives agent status by parsing OSC titles
out of the **PTY stream** (`pty-transport.ts` → `runtimePaneTitlesByTabId` →
`detectAgentStatusFromTitle`). It does **not** consume the server event bus, so
`/api/sessions/{id}/hook` → `agent.hook` events never reach the desktop. A hook
subsystem could not have driven the desktop spinner without also building a
bus→renderer bridge — and it wasn't needed at all (see below). It also carried
real risk (relocating `CODEX_HOME` on every Codex launch). All of it was
reverted.

## The actual root cause (confirmed with a captured live session)

Codex **does** emit braille-spinner OSC titles while working — captured from the
real Codex pane log (`~/Library/Caches/agentum/sessions/<id>.log`):

```
⠼ testi   ⠴ testi   ⠦ testi   ⠧ testi   …   testi   ⠦ testi   …   testi
```

But Codex **interleaves a bare, status-less `testi` frame between the braille
frames while still working** (and `testi` is also its idle title). Every bare
frame makes `detectAgentStatusFromTitle("testi")` return `null`, so the worktree
status collapses working→idle for that instant. The spinner flickers off and
never reads as steadily working. Claude never does this — every frame is
`⠋ Claude Code` — so it's rock-steady. This is a **title-pipeline robustness
bug, not a missing-signal bug.** It is agent-agnostic: any agent whose spinner
animation drops a status-less frame is affected.

## The fix

`crates/agentum-desktop/ui/src/components/terminal-pane/pty-transport.ts` —
`applyObservedTerminalTitle` now **holds a transient working→non-working title**
for `WORKING_TITLE_HOLD_MS` (300ms) when the incoming frame is exactly the
spinner-stripped form of the current working title
(`normalized === clearWorkingIndicators(lastEmittedTitle)`):

- A returning working frame within the window cancels the hold → the blip is
  absorbed and the spinner stays steady.
- A frame that survives the window commits → a genuine turn-end still clears the
  spinner (after a barely-perceptible 300ms).
- The scope is tight: a *different* idle title (Claude's `* Claude done` vs
  `. Claude working`) is a real completion and applies immediately, so distinct
  transitions and completion notifications are never delayed.

Tests:
- `pty-transport-codex-spinner-flicker.test.ts` (new) — reproduces the exact
  Codex braille/bare interleave; asserts the blip is absorbed and that a
  sustained bare title still commits to idle.
- `pty-transport-pi-coalesce.test.ts` — updated to allow the intentional hold
  before the trailing idle lands.

## Not covered (separate, larger problem)

Agents that emit **no status title at all** (e.g. cursor-agent's native title is
the literal `Cursor Agent`, which `applyObservedTerminalTitle` deliberately
drops) get no spinner from this fix. Those genuinely need a status signal the
agent doesn't provide — a per-agent hook or synthesized title — which requires
verifying each CLI's hook contract first. Out of scope here; tracked as future
work. (The `:8822` hardcoded hook URL in `sessions.rs` — wrong for the embedded
desktop server's ephemeral port — was also noticed during this investigation and
left for a separate change.)
