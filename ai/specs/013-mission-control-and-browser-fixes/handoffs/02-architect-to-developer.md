# Handoff 02 — Architect → Developer (spec 013)

- **Spec:** 013-mission-control-and-browser-fixes
- **From:** Architect  **To:** Developer
- **Date:** 2026-07-08
- **Gate:** Architect gate PASS — `architecture.md` written.
- **Note:** the architect subagent was stopped mid-run (misjudged liveness);
  the orchestrator authored `architecture.md` from the PM handoff's complete,
  develop-verified design. No design gap — it's implementation-ready.

## Read
- `ai/specs/013-mission-control-and-browser-fixes/architecture.md` (boundaries,
  per-feature design, build order, the F2 spike, test mapping)
- `ai/specs/013-mission-control-and-browser-fixes/spec.md` (ACs + locked decisions)

## Do this first (non-negotiable)
1. **Create a fresh worktree off `origin/develop`** — this spec's worktree is 59
   commits behind and missing the current code:
   `git worktree add ../agentum-013-fixes -b fix/mission-control-and-browser-fixes origin/develop`
   Copy the sherpa/onnx dylibs into `target/release/` if `cargo check -p
   agentum-desktop` complains (project memory).
2. **Re-locate every `file:line` anchor** in `architecture.md` on `origin/develop`
   before editing — they *will* have drifted.

## Build order (one gated slice each)
- **F1 — Mission Control close redirect** (do first; isolated store):
  pure `viewAfterWorktreeClose(removedActiveWorktree, currentView)` +
  cascade-stamp `activeView:'activity'` in `worktrees.ts` (`removeWorktree`,
  batch, `setActiveWorktree(null)`) + `sleep-worktree-flow.ts`. **Enumerate every
  active-worktree-nulling path** and prove the set is complete (that's the AC1/AC2
  risk). Do **not** add the `App.tsx` effect unless a null-path the cascade can't
  reach is found.
- **F3 — Browser paste** (do second; contained): new `browser.insertText` wire
  message (`shared/browser-screencast-protocol.ts`, `cdp-screencast-client.ts`),
  `onPaste`/`ClipboardEvent` in `AgentBrowserScreencastPane.tsx` (NO `readText()`),
  `InputCommand::InsertText` + parse + `Input.insertText` dispatch in
  `cdp_screencast.rs`. Agent-driver path untouched.
- **F2 — Browser viewport + clicks** (do last; carries the spike): pure
  `screencast-geometry.ts` (contain content box + `clientToDevicePoint`, both bar
  orientations) wrapped by `toDevicePoint`; re-send `sendViewport` after settled
  size + force a re-capture. **Run the F2 spike** (architecture §5) to decide
  UI-only relayout vs a bounded server re-capture — no timer poll (principle 3).

## Verify (the gate)
- `verify.sh`: `cargo test -p agentum-server --lib` + `bunx vitest run` (pure
  modules) + `bun run build` (Vite typecheck proxy — bare `tsc` fails on
  `shared/*`).
- Per-AC assertions: see `architecture.md` §6.
- `qa.sh` scenarios (browser fills + click + paste; close-workspace Mission
  Control full-width) are browser-QA — may be human/Mateo-gated at staging.

## Commit rule
Work only in the new `origin/develop` worktree; stage only your files (never
`git add -A` in a shared checkout). Commit per slice.

## Invariants (do not regress)
Push-streaming reuse of the input WS channel (principle 3) · agent-driver
`Input.insertText` untouched · redirect only on **active** worktree close.
