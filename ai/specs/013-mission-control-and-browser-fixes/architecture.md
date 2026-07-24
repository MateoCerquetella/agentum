# Architecture — Spec 013 (Mission Control after workspace-close + browser fixes)

- **Author:** Orchestrator (architect subagent stopped mid-run — see STATE deviation note)
- **Date:** 2026-07-08
- **Inbound:** `handoffs/01-pm-to-architect.md` (PM gate 9/9 PASS, locked decisions)

> **All anchors below were verified on `origin/develop`.** This worktree is 59
> commits behind and its copies are stale. **The developer MUST create a fresh
> worktree off `origin/develop`** (`git worktree add ../agentum-013-fixes -b
> fix/mission-control-and-browser-fixes origin/develop`) and re-confirm every
> line number before editing. Line numbers *will* have drifted.

---

## 1. Boundaries

### F1 — Mission Control close redirect (pure store; UI only)
**Changes:**
- `crates/agentum-desktop/ui/src/store/slices/worktrees.ts` — the three
  active-worktree-nulling return objects (`removeWorktree` `~:1349`, batch
  `~:740`, `setActiveWorktree(null)` `~:1978`).
- `crates/agentum-desktop/ui/src/components/sidebar/sleep-worktree-flow.ts` —
  the sleep/close path that calls `setActiveWorktree(null)`.
- (New) a tiny pure helper module for the redirect decision + its test.

**Must NOT change:** `lib/right-sidebar-visibility.ts` (already correct —
`'activity'` suppressed, `'terminal'` not), `App.tsx` render forks
(`:1751`/`:1758` stay as-is; the `:1758` fallback becomes rarely-hit but is
harmless defense-in-depth), `openActivityPage` (`ui.ts:1097`) semantics.

### F2 — Browser viewport + contain-aware clicks (screencast pane; UI only)
**Changes:**
- `crates/agentum-desktop/ui/src/components/browser-pane/AgentBrowserScreencastPane.tsx`
  — `toDevicePoint` (`:63`), the `sendViewport` timing (`:140`, initial `:275`,
  ResizeObserver `:309-329`), first-frame re-capture nudge.
- (New) a pure geometry module (`screencast-geometry.ts`) + its test.
- Possibly `crates/agentum-server/src/cdp_screencast.rs` **only if** the F2 spike
  concludes an explicit server-side re-capture (`Page.startScreencast` re-arm /
  one-shot `Page.captureScreenshot`) is required.

**Must NOT change:** `remote-browser-frame-style.ts` (dead for this surface —
it feeds the legacy native pane, not the screencast canvas), the `object-contain`
canvas CSS (the fix mirrors it, doesn't remove it), the agent-driver path.

### F3 — Browser paste (new input verb; UI + server)
**Changes:**
- `AgentBrowserScreencastPane.tsx` — add an `onPaste` handler (canvas), route to a
  new `browser.insertText` message; the Cmd/Ctrl+V branch in `onKeyDown` (`:404`)
  must route to `browser.insertText` too, never `browser.keypress` (`:420`).
- `crates/agentum-desktop/ui/src/components/browser-pane/remote-browser-keyboard.ts`
  — extend to recognize paste and build the insert message (pure, testable).
- `crates/agentum-desktop/ui/src/shared/browser-screencast-protocol.ts` +
  `runtime/cdp-screencast-client.ts` — the new `browser.insertText` wire message.
- `crates/agentum-server/src/cdp_screencast.rs` — `InputCommand::InsertText{text}`
  (`enum` `:185`), a `"browser.insertText"` arm in `parse_input_message` (`:243`),
  and an `Input.insertText` dispatch in `input_command_to_cdp` (`~:296`).

**Must NOT change:** `cdp_driver.rs` (`Input.insertText` at `:550` stays — F3 adds
a *parallel* human arm; two paths, one CDP verb), the existing `browser.mouse*` /
`browser.keypress` arms.

---

## 2. Design per feature

### F1 — cascade-stamp `activeView:'activity'`
Extract the decision as a pure function so AC1 is a unit test, not a store
integration test:

```ts
// store/slices/… (new pure helper, e.g. worktree-close-view.ts)
// Returns the view to land on after a worktree close.
export function viewAfterWorktreeClose(
  removedActiveWorktree: boolean,
  currentView: ActiveView
): ActiveView {
  // Only redirect when the ACTIVE worktree was the one closed; otherwise the
  // user stays where they are (closing a background worktree must not yank them).
  return removedActiveWorktree ? 'activity' : currentView
}
```

Then each cascade return object adds `activeView: viewAfterWorktreeClose(
removedActiveWorktree, s.activeView)` alongside its existing
`activeWorktreeId: … ? null : …`. This is atomic (single store transition — no
one-frame flash of the broken `terminal && !activeWorktreeId` fallback) and
satisfies AC1 directly.

**Exhaustiveness is the risk (see §5).** The developer must enumerate *every*
path that nulls the active worktree on `origin/develop` — the three in
`worktrees.ts` + `sleep-worktree-flow.ts` are the known set; confirm there are no
others (search `activeWorktreeId: null` and `setActiveWorktree`). The `App.tsx`
catch-all effect stays **unbuilt** unless a path is found the cascade can't reach.

### F2 — first-frame viewport + contain-aware clicks
Two coupled fixes sharing the `object-contain` geometry:

**(a) Contain-aware `toDevicePoint` (pure, unit-tested).** Extract the mapping to
`screencast-geometry.ts`:

```ts
// The letterboxed content box of an object-contain image inside its element box.
export function containContentBox(boxW: number, boxH: number, frameW: number, frameH: number):
  { offsetX: number; offsetY: number; contentW: number; contentH: number }
// Map a client point (relative to the element box) into device/frame pixels,
// clamping to the content box so a click on a bar maps to the nearest edge (or is dropped).
export function clientToDevicePoint(
  clientX: number, clientY: number, box: DOMRectLike, frameW: number, frameH: number
): { x: number; y: number } | null
```

`toDevicePoint` (`:63`) becomes a thin wrapper over `clientToDevicePoint`. Test
both bar orientations (frame wider than box → bars left/right; frame taller →
bars top/bottom) + the no-letterbox case (exact map).

**(b) First-frame correctness.** Re-send `sendViewport()` after the container has
its settled, non-zero size (guard the existing zero-rect check at `:151`) **and**
force a re-capture so the idle page repaints at the pane aspect. The *mechanism*
is the spike (§5): preferred = a metrics-override relayout that triggers a fresh
frame; permitted fallback = an explicit server re-capture. **No timer poll.**
Reuse the existing input WS channel (`sendViewport` already does — principle 3).

### F3 — `browser.insertText` → CDP `Input.insertText`
**UI (pure part testable):**

```ts
// remote-browser-keyboard.ts — pure builder
export function getRemoteBrowserInsertText(text: string): { method: 'browser.insertText'; params: { text: string } } | null
// null for empty text; trims nothing (paste is verbatim)
```

`onPaste` reads `e.clipboardData.getData('text')` → `getRemoteBrowserInsertText`
→ `sendInput`. The Cmd/Ctrl+V branch in `onKeyDown` calls `preventDefault()` and
lets the browser's native paste fire the `onPaste` (or reads from the event) — it
must **not** emit `browser.keypress {key:"v"}`. **No `navigator.clipboard.
readText()`** (webview permission/blocking risk — PM-locked).

**Server (Rust, unit-tested):**
```rust
// cdp_screencast.rs
enum InputCommand { /* … */ InsertText { text: String } }
// parse_input_message: "browser.insertText" => Some(InsertText { text: params.text })
// input_command_to_cdp: InsertText { text } => Input.insertText { text }
```
Mirror the `Input.insertText` param shape used at `cdp_driver.rs:550`.

---

## 3. Build order

**F1 → F3 → F2.**
1. **F1** first — isolated store change, zero coupling, pure-unit-testable, lowest
   risk. Lands the Mission Control fix independently (per the independence note,
   it must not wait on the browser work).
2. **F3** next — a contained new input verb (UI message + Rust arm), fully
   unit-testable on both sides, no empirical unknown.
3. **F2** last — carries the first-frame re-capture spike (the one empirical
   risk); doing it last means its uncertainty never blocks F1/F3.

F2 and F3 both edit `AgentBrowserScreencastPane.tsx` — sequencing F3 before F2
keeps their diffs from colliding; coordinate the `onKeyDown`/handler region.

---

## 4. Tradeoffs / guidance

- **Redirect = cascade-stamp, not an effect** (PM-locked). The atomicity is worth
  more than the effect's null-path safety net; the developer just has to prove
  exhaustiveness. If review finds a missed null path, add the `App.tsx` effect as
  a targeted patch — don't pre-build it.
- **Clamp vs drop on a letterbox click.** Prefer *clamp to the content box* (a
  near-edge click still hits the page) over dropping; but after F2(b) lands, bars
  ≈ 0 so this is belt-and-suspenders. Keep both — AC3 and AC4 back each other up.
- **Where the paste verb lives.** In `cdp_screencast.rs` alongside the other
  `browser.*` human input arms — not in `cdp_driver.rs` (agent path). One CDP
  verb, two independent call sites.
- **Device-scale (`--force-device-scale-factor=2`) stays out of scope** — it's a
  sharpness nit, aspect-preserving, not the letterbox cause. Do not touch it here.

---

## 5. Risks & the F2 spike

- **F2 first-frame re-capture (empirical spike — do this before finalizing F2b).**
  Against a real, fully-loaded *static* page: does a metrics-override relayout
  (`Emulation.setDeviceMetricsOverride` at the settled pane size) trigger a fresh
  screencast frame? If YES → UI-only fix. If NO → add a bounded one-shot
  server-side re-capture (`Page.startScreencast` re-arm or `Page.captureScreenshot`)
  in `cdp_screencast.rs`. **Forbidden either way:** a timer / `capture-pane`-style
  full-frame poll (principle 3). This is the crux of the "works only after
  remount" bug.
- **F1 exhaustive cascade coverage.** A missed active-worktree-nulling path
  reproduces the bug. Enumerate all of them on `origin/develop` before claiming
  AC1/AC2 green.
- **Stale-worktree anchor drift (all features).** Every `file:line` here is from
  `origin/develop`; re-locate before editing. Build on a fresh `origin/develop`
  worktree. Note: a fresh `cargo check -p agentum-desktop` needs the sherpa/onnx
  dylibs copied from `target/release/` (see project memory).
- **Do not regress the agent-driver `Input.insertText`.** F3 adds a parallel arm.

---

## 6. Test strategy (gate mapping)

| AC | Gate | Assertion |
|----|------|-----------|
| AC1 | `verify.sh` (vitest) | `viewAfterWorktreeClose(true, 'terminal') === 'activity'`; `(false, 'terminal') === 'terminal'`; store cascades return `activeView:'activity'` when the active worktree is nulled. |
| AC2 | `qa.sh` | After close w/ sidebar open: `canShowRightSidebarForView('activity') === false` (pure) + `StatsPane` renders 3 columns + content box `scrollWidth <= clientWidth` (QA). |
| AC3 | `qa.sh` | First browser open: object-contain content box matches canvas box within ≤2px both axes (edge-row screenshot = page content, not black). |
| AC4 | `verify.sh` (vitest) | `screencast-geometry` pure fns: correct device point for both bar orientations + exact (no-letterbox) case; bar-region click clamps/drops. |
| AC5 | `verify.sh` (cargo + vitest) | Rust: `parse_input_message("browser.insertText")` → `InsertText`; `input_command_to_cdp(InsertText)` → `Input.insertText`. UI: paste event / Cmd+V → `browser.insertText {text}` message, never `browser.keypress {key:"v"}`. |

`verify.sh`: `cargo test -p agentum-server --lib` + `bunx vitest run` (pure
modules) + `bun run build` (Vite = typecheck proxy; bare `tsc` can't resolve
`shared/*`). `qa.sh`: the two browser-QA scenarios (browser fills + click + paste;
close-workspace Mission Control full-width).
