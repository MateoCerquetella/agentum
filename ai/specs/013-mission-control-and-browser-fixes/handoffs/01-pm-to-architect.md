# Handoff 01 — PM → Architect (spec 013)

- **Spec:** 013-mission-control-and-browser-fixes
- **From:** PM  **To:** Architect
- **Date:** 2026-07-08
- **Gate:** PM gate **9/9 PASS** — GO to architect (no send-back).

## What's decided (don't re-open)

Two independent bugs, bundled per Mateo's explicit "all in one spec" ask, sliced
into 3 independently-gated harness features:

- **F1 — Mission Control close redirect** (pure store): stamp `activeView:
  'activity'` in the active-worktree-nulling cascades so Mission Control renders
  through the right-sidebar-**suppressed** `activity` slot, not the
  `terminal && !activeWorktreeId` fallback. Root cause confirmed:
  `RIGHT_SIDEBAR_SUPPRESSED_VIEWS` has `'activity'` but not `'terminal'`, so the
  post-close fallback keeps the right sidebar mounted and squeezes `StatsPane`'s
  `grid-cols-3`.
- **F2 — browser viewport + click** (CDP screencast pane): fix the first-frame
  letterbox (viewport sent/re-captured after settled size) **and** make
  `toDevicePoint` object-contain-aware. These two symptoms share one geometry, so
  they co-gate as one feature (belt-and-suspenders — keep both).
- **F3 — browser paste** (new input arm): `onPaste` → `browser.insertText` →
  CDP `Input.insertText`. Currently unimplemented; Cmd+V types literal "v".

**Locked decisions (were open questions):**
1. Paste text comes from the `onPaste` `ClipboardEvent`; **no**
   `navigator.clipboard.readText()`.
2. Redirect = cascade-stamp `activeView:'activity'` (atomic); the `App.tsx`
   catch-all effect is optional contingency, **not** gated.
3. First-frame re-capture mechanism is an **F2 empirical spike / architecture
   risk** — not a blocker; no timer poll (principle 3).

## Architect's job

Produce `ai/specs/013-mission-control-and-browser-fixes/architecture.md`:
boundaries, tradeoffs, risks, and the concrete edit-site map per feature.

## Must physically verify (worktree is stale)

⚠️ **This worktree is 59 commits behind `origin/develop` (missing 009–011).**
All anchors were verified against `origin/develop`, not local copies. **Design
against fresh `origin/develop`** and re-confirm line numbers, especially:

- The **complete set** of active-worktree-nulling paths for F1 —
  `worktrees.ts:1349` (`removeWorktree`), `:740` (batch), `:1978`
  (`setActiveWorktree(null)`), and `sleep-worktree-flow.ts`. A missed path is
  exactly what the deferred `App.tsx` effect would have caught — so confirm these
  four are exhaustive on develop.
- `cdp_screencast.rs` `InputCommand` (`:185`) / `parse_input_message` (`:243`) /
  `input_command_to_cdp` (`~:296`) for the F3 arm; reuse the `Input.insertText`
  shape from `cdp_driver.rs:550`.
- `AgentBrowserScreencastPane.tsx` `toDevicePoint` (`:63`), `sendViewport`
  (`:140`), ResizeObserver (`:309-329`), `object-contain` canvas (`~:495`).

## Invariants to protect

- Push-based streaming, never poll (principle 3) — the viewport re-send/repaint
  must reuse the existing input WS channel.
- Agent-driver CDP path (`cdp_driver.rs`) stays untouched — F3 adds a **parallel**
  human arm (two paths, one CDP verb).
- F1 redirects **only** when the *active* worktree is closed.
