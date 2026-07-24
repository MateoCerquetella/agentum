# Spec 013 — Mission Control after workspace-close + in-app browser fixes

- **Number:** 013
- **Status:** Architect
- **Surface:** `crates/agentum-desktop/ui` (React) + `crates/agentum-server/src` (CDP screencast bridge)
- **Author:** Mateo (via /sdd-spec)
- **Date:** 2026-07-08

> **⚠️ Base-branch note (read before implementing).** This spec was authored in
> the `fix-after-closing-workspace` worktree, which is **59 commits behind
> `origin/develop`** and does not contain specs 009–011. All `file:line` anchors
> below were verified against **`origin/develop`** (not this worktree's stale
> copies). **Implement on a fresh worktree based off `origin/develop`.** Spec
> number **012** is already claimed by a concurrent draft ("pick work item +
> status sync"), so this is **013**.

## Problem

Two unrelated surfaces are broken in the desktop app, and Mateo hit both in one
session:

1. **After closing a workspace, Mission Control renders squished / wrong width
   while the left sidebar is open.** Closing the active workspace drops you back
   to Mission Control, but the content is compressed to one side instead of
   filling the pane.
2. **The in-app browser is unusable on first open.** Buttons don't respond to
   clicks, the page doesn't fill the pane (two black bars, top and bottom), and
   pasting (Cmd/Ctrl+V) does nothing. Popping the browser out and re-entering
   makes the clicks and sizing start working — but that workaround shouldn't be
   necessary, and paste stays broken regardless.

## Goal

Make Mission Control render correctly (full width, no leftover workspace chrome)
the instant a workspace is closed, and make the in-app browser usable on first
open — clicks land, the frame fills the pane with no letterbox, and paste inserts
clipboard text — without the pop-out/re-enter dance.

## Users / personas

The operator (Mateo) driving agents from the desktop cockpit: they close a
finished workspace and land on a broken-looking Mission Control, and they open
the in-app browser to check a running web app but can't click, can't see the
whole page, and can't paste a URL or credentials.

## Acceptance criteria

**Mission Control (workspace-close):**

1. Closing the **active** workspace (delete via `removeWorktree`, sleep/close via
   `setActiveWorktree(null)` / `runSleepWorktrees`) leaves the app on
   `activeView === 'activity'` — i.e. Mission Control renders through the
   right-sidebar-**suppressed** `activity` slot (`App.tsx:1751`), **not** the
   `terminal && !activeWorktreeId` fallback (`App.tsx:1758`). Testable as pure
   store logic: the close cascade returns `activeView: 'activity'` whenever it
   nulled the active worktree.
2. After closing a workspace **with the left sidebar open**, Mission Control
   occupies the full content width: `canShowRightSidebarForView('activity') ===
   false` (right sidebar not mounted), `StatsPane` renders its 3 columns
   (`grid-cols-3`, `StatsPane.tsx:88`), and the content box has **no horizontal
   scrollbar** (`scrollWidth <= clientWidth`). The `canShowRightSidebarForView`
   check is pure-testable; the 3-column render and no-overflow are observable in
   browser QA.

**In-app browser (CDP screencast pane, `AgentBrowserScreencastPane.tsx`):**

3. On the **first** open of the in-app browser, the streamed frame fills the pane
   with **no black bars** top/bottom — no pop-out/re-enter required. The
   object-contain content box matches the canvas box within **≤2px on both axes**
   on the first painted frame (letterbox offset ≈ 0), so the top and bottom edge
   rows are page content, not black. The pane re-sends `sendViewport` after the
   container has its settled size **and** re-requests a frame once metrics are
   applied. Observable in browser QA (edge-row screenshot).
4. Clicking a button/link in the browser on first load **activates it**:
   `toDevicePoint` maps client coordinates against the letterboxed **content
   box** (object-contain math), so a click is correct even during a transient
   aspect mismatch and never lands on a black bar. Testable: extracted pure
   contain-mapping function returns the on-image device point for a given
   canvas box + frame aspect (both bar orientations).
5. Pressing **Cmd/Ctrl+V** in the browser pane inserts the clipboard **text**
   into the focused page field (not a literal "v"). A new
   `browser.insertText`-style input message is parsed in `cdp_screencast.rs`
   (`InputCommand` + `parse_input_message`) and dispatched as CDP
   `Input.insertText`. Testable: Rust unit test for the new parse+map arm; UI
   unit test that a paste event / Cmd+V produces a `browser.insertText` message
   with the clipboard text rather than `browser.keypress {key:"v"}`.

## Scope & non-goals (YAGNI)

- **In:**
  - Redirect `activeView → 'activity'` on active-workspace close (Increment A).
  - Fix the screencast pane's initial letterbox + contain-aware click mapping
    (Increment B).
  - Implement a paste path for the human screencast browser (Increment C).
- **Out:**
  - Rich clipboard (images, HTML) paste — **text only** for now.
  - The device-scale mismatch (`--force-device-scale-factor=2` vs the pane's 1×
    assumption, `cdp_browser.rs:491`) — a real sharpness nit but **not** the
    letterbox cause; note it, don't fix it here.
  - The dormant/native browser path (`RemoteBrowserPagePane`, `NativeBrowser…`)
    and `remote-browser-frame-style.ts` — the **live** surface is
    `AgentBrowserScreencastPane.tsx` with a hardcoded `object-contain` canvas.
  - Any change to which workspace you land on after close (existing behavior
    already falls back to Mission Control — we only fix its **layout**).
  - The agent-driver CDP path (`cdp_driver.rs`) — untouched; we add a **parallel**
    human input arm.

## Reuse vs build (ground in code — verified on `origin/develop`)

### Already exists — do NOT rebuild

- **`openActivityPage`** (`store/slices/ui.ts:1097`, impl sets `activeView:
  'activity'`; declared `:554`) — the one action that switches to Mission
  Control. Reuse it (or its effect) rather than hand-setting `activeView`.
- **Right-sidebar suppression** (`lib/right-sidebar-visibility.ts:6-25`) —
  `'activity'` is already in `RIGHT_SIDEBAR_SUPPRESSED_VIEWS` (line 9);
  `'terminal'` is not. This is *why* the two render paths differ; landing on
  `activity` is the whole fix. No new gate needed.
- **Close cascades** (`store/slices/worktrees.ts`): `removeWorktree` nulls the
  active worktree at `:1349`, batch at `:740`, `setActiveWorktree(null)` at
  `:1978`. These are the exact return objects to also stamp `activeView`.
- **`sendViewport` + ResizeObserver** (`AgentBrowserScreencastPane.tsx:140`,
  initial call `:275`, observer `:309-329`) — the viewport-sync channel already
  exists and reuses the input socket (no re-subscribe). Extend the *timing*, do
  not rebuild the channel (respect the push-streaming invariant).
- **`Input.insertText` over CDP** already implemented on the agent-driver path
  (`cdp_driver.rs:550`). Reuse the same CDP call shape for the human bridge.
- **`InputCommand` + `parse_input_message` + `input_command_to_cdp`**
  (`cdp_screencast.rs:185 / 243 / ~296`) — the human input protocol. Add one
  arm; keep the existing `browser.mouse*` / `browser.keypress` arms intact.
- **Key serializers** (`remote-browser-keyboard.ts`
  `getRemoteBrowserKeypressKey` / `getRemoteBrowserKeyboardShortcut`) — extend to
  recognize paste; `onKeyDown` lives at `AgentBrowserScreencastPane.tsx:404`,
  sends `browser.keypress` at `:420`.

### Build new

- **Close → activity redirect**: stamp `activeView: 'activity'` in the close
  cascades when the active worktree is nulled (`worktrees.ts:1349/740/1978`,
  `sleep-worktree-flow.ts`). **PM-locked:** the cascade-stamp is the
  implementation and the F1 gate (AC1) — it's atomic (no one-frame `terminal`
  fallback flash) and directly unit-testable. The `App.tsx` catch-all effect
  (`openActivityPage()` when `activeView === 'terminal' && !activeWorktreeId`) is
  **optional** hardening, **not gated** — add it only if a null-path the cascade
  can't cover is found.
- **Contain-aware `toDevicePoint`**: extract the mapping into a pure function
  that computes the object-contain content rect (offset + scale) from the canvas
  box and the frame aspect, then maps into device pixels. Handles bars on either
  axis. Unit-tested.
- **First-frame viewport correctness**: re-send `sendViewport()` after the
  container's settled size is known and/or after the first frame arrives, and
  force a repaint/relayout so a static (idle) page re-captures at the pane aspect
  (a fully-loaded page emits ~one frame, so a late metrics override alone leaves
  the stale frame painted — this is the root of "works only after remount").
- **Paste path** (**PM-locked:** paste text comes from the `onPaste`
  `ClipboardEvent` via `clipboardData.getData('text')`; `navigator.clipboard.
  readText()` is **not** used — webview permission/blocking risk):
  - UI: an `onPaste` handler on the canvas is the source of truth. A Cmd/Ctrl+V
    branch in `onKeyDown` may act as a convenience trigger, but the text must come
    from the `ClipboardEvent`, never `readText()`. It sends a `browser.insertText`
    message instead of `browser.keypress`.
  - Server: `InputCommand::InsertText { text }` + a `browser.insertText` arm in
    `parse_input_message` + a `Input.insertText` dispatch in `input_command_to_cdp`
    (`cdp_screencast.rs`).

## Risks & invariants

- **Push-based streaming, never poll** (architecture principle 3). The viewport
  re-send and repaint-nudge must reuse the existing input WS channel (as
  `sendViewport` already does) — do **not** reintroduce a `capture-pane`-style
  or timer-based full-frame poll to "force" frames.
- **Don't regress the agent-driver path.** `cdp_driver.rs` `Input.insertText`
  stays as-is; the human bridge gets its own arm. Two paths, one CDP verb.
- **Clipboard permissions in the Tauri webview (PM-locked).** Paste text comes
  from the `onPaste` `ClipboardEvent` (synchronous, no permission). Do **not** use
  `navigator.clipboard.readText()` (may prompt / be silently blocked in the
  webview).
- **First-frame re-capture is an empirical CDP unknown (Increment B spike).**
  Whether a metrics-override relayout re-captures a fully-idle static page, or
  whether an explicit `Page.startScreencast` re-arm / bounded one-shot
  `Page.captureScreenshot` is needed, must be confirmed against a real static page
  during F2. Either mechanism is in-scope — but **no** timer / `capture-pane` poll
  (principle 3). This is the crux of the "works only after remount" bug.
- **Contain math must match the canvas CSS.** The mapping fix is only correct if
  it mirrors the actual `object-contain` layout (`AgentBrowserScreencastPane.tsx:
  ~495`); if the letterbox fix (AC-3) fully removes bars, AC-4's math is
  belt-and-suspenders — keep both so a transient mismatch never mis-routes a click.
- **Redirect scope.** Only redirect when the **active** worktree is closed;
  closing a background worktree must not yank the user to Mission Control.

## Harness wiring (the gate)

- **feature_list.json entries (3 increments):**
  - `F1 — mission-control-close-redirect`: close cascades stamp
    `activeView:'activity'`; Mission Control renders right-sidebar-suppressed
    after close.
  - `F2 — browser-viewport-and-click`: no letterbox on first open; contain-aware
    click mapping.
  - `F3 — browser-paste`: `browser.insertText` UI→CDP path; Cmd/Ctrl+V pastes.
  - **Independence:** F1 (Mission Control) and F2+F3 (browser) are separate bug
    domains sharing this spec doc only — a block on one must **not** hold the
    other's landing; track them as separate checkable items on the issue.
- **`verify.sh` asserts (unit gate):**
  - `cargo test -p agentum-server --lib` — new `InputCommand::InsertText`
    parse + `input_command_to_cdp` map arm.
  - `bunx vitest` (UI pure modules) — contain-mapping function (both bar
    orientations), close-cascade returns `activeView:'activity'`, paste-key
    detection produces `browser.insertText`.
  - `bun run build` (Vite) — build stands in for typecheck (bare `tsc` can't
    resolve the `shared/*` alias; see project memory).
- **`qa.sh` asserts (browser QA gate):**
  1. Open the in-app browser → frame fills the pane, **no black bars** → click a
     visible button → it activates → Cmd/Ctrl+V into a field → clipboard text
     appears.
  2. Open a workspace, then close it with the left sidebar open → Mission Control
     fills the width, the right sidebar is absent, no horizontal overflow.

## Locked decisions (were open questions — resolved by PM, autonomous)

1. **Paste source → `onPaste` `ClipboardEvent` (text-only).** Primary path reads
   `clipboardData.getData('text')` → `browser.insertText`; a Cmd/Ctrl+V branch may
   trigger it but must route to `browser.insertText`, never `browser.keypress`.
   No `navigator.clipboard.readText()`. (Folded into *Reuse vs build › Paste path*
   and *Risks*.)
2. **Redirect mechanism → cascade-stamp `activeView:'activity'`.** Atomic, no
   one-frame flash, and it *is* the F1 gate (AC1). The `App.tsx` catch-all effect
   is optional contingency, not gated. (Folded into *Build new › Close → activity
   redirect*.)
3. **First-frame nudge → carried as an architecture risk, not a blocker.** The
   *outcome* is locked by AC3; the *mechanism* (relayout-repaint vs an explicit
   `Page.startScreencast` re-arm / one-shot capture) is an F2 empirical spike — no
   timer poll. (Folded into *Risks & invariants*.)

_No open questions remain for the human — this handoff is unblocked._
