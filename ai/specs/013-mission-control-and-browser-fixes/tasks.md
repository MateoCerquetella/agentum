# Tasks — Spec 013

Branch: `fix/mission-control-and-browser-fixes` (off `origin/develop` @ `75d03eaa`).

## F1 — Mission Control close redirect ✅ CODE-COMPLETE + GREEN

Redirect `activeView → 'activity'` when the **active** worktree is closed, so
Mission Control renders through the right-sidebar-suppressed `activity` slot
instead of the squeezed `terminal && !activeWorktreeId` fallback.

**Files:**
- `crates/agentum-desktop/ui/src/store/slices/worktree-close-view.ts` (new) —
  pure `viewAfterWorktreeClose(removedActiveWorktree, currentView)`; redirects to
  `'activity'` **only** from the `'terminal'` view (never yanks settings/tasks/…).
- `crates/agentum-desktop/ui/src/store/slices/worktree-close-view.test.ts` (new)
  — 4 cases (redirect / background-close no-op / already-activity / non-terminal
  views untouched).
- `crates/agentum-desktop/ui/src/store/slices/worktrees.ts` — stamped 3 cascade
  return-objects: batch remove (`~:729`), `removeWorktree` (`~:1351`), and the
  central `setActiveWorktree(null)` branch (`~:1980`) — the last covers 5 close
  callers (Terminal.tsx ×2, sleep-worktree-flow, TerminalPaneOverlayLayer,
  terminal-tab-actions, useTabGroupWorkspaceModel).
- `crates/agentum-desktop/ui/src/store/slices/tabs.ts` — stamped the
  `closeUnifiedTab` deactivate branch (`~:720`) — the 4th nulling path (found
  during anchor location; not in the original spec's known set).

**Exhaustiveness:** grepped every `activeWorktreeId: null` production site; all
covered (worktrees.ts batch/removeWorktree/setActiveWorktree + tabs.ts
closeUnifiedTab). `setActiveWorktree(null)` is the central chokepoint for the
component-level close callers. Selecting a worktree restores `activeView:'terminal'`
(`repos.ts:629`), so no sticky-'activity' hazard.

**Gate:**
- `bunx vitest run worktree-close-view.test.ts` → **4/4 pass**.
- `bun run build` (Vite) → **green** (1m45s).
- Regression: `store-session-cascades` + `tabs` + `worktrees` tests → **192/193
  pass**; the 1 failure (`drops browser tabs for invalid worktrees`,
  `webviewClose` on undefined) is **pre-existing** — verified failing identically
  with F1 edits reverted (Tauri-API-in-jsdom baseline).

**Deviation:** the `viewAfterWorktreeClose` helper adds a `currentView === 'terminal'`
guard beyond the architecture's simpler form — prevents yanking users off
settings/tasks/projects when a background process nulls the active worktree.

## F3 — Browser paste ✅ CODE-COMPLETE + GREEN

New `browser.insertText` input verb: `onPaste` ClipboardEvent → `browser.insertText`
→ CDP `Input.insertText` (trusted paste). Cmd/Ctrl+V no longer types a literal "v".

**Files:**
- `crates/agentum-server/src/cdp_screencast.rs` — `InputCommand::InsertText{text}`
  variant + `is_human_action` arm + `"browser.insertText"` parse arm + dispatch to
  CDP `Input.insertText` (reuses the trusted-insert primitive the agent-driver fill
  path uses, `cdp_driver.rs:550`). Agent-driver path untouched. + Rust test
  `insert_text_parses_and_maps_to_cdp_insert_text`.
- `crates/agentum-desktop/ui/src/components/browser-pane/remote-browser-keyboard.ts`
  — pure `getRemoteBrowserInsertText(text)` builder + `isRemoteBrowserPasteShortcut`.
- `crates/agentum-desktop/ui/src/components/browser-pane/remote-browser-keyboard.test.ts`
  — paste-chord detection + insertText builder (empty text dropped).
- `crates/agentum-desktop/ui/src/components/browser-pane/AgentBrowserScreencastPane.tsx`
  — `onPaste` handler (text-only `ClipboardEvent`, NO `navigator.clipboard.readText()`)
  wired on the canvas; `onKeyDown` short-circuits Cmd/Ctrl+V so the native paste
  fires (no preventDefault, no literal-"v" keypress). Transport `sendInput(method,
  params)` is generic — no protocol/client change needed.

**Gate:** UI `remote-browser-keyboard.test` → **5/5**; `bun run build` → **green**;
`cargo test -p agentum-server --lib cdp_screencast` → **16/0** (incl. the new test).

**QA-deferred (needs the running app + a real CDP page):** the live "Cmd+V pastes
into a focused page field" is a `qa.sh` / installed-app check (Mateo).

## F2 — Browser viewport + contain-aware clicks ✅ CODE-COMPLETE + GREEN

Fix first-open letterbox + click mis-routing (they share the object-contain geometry).

**Files:**
- `crates/agentum-desktop/ui/src/components/browser-pane/screencast-geometry.ts`
  (new) — pure `containContentBox()` + `clientToDevicePoint()` (object-contain
  content box; maps a click into device pixels; **drops bar clicks** so they're
  never mis-routed).
- `crates/agentum-desktop/ui/src/components/browser-pane/screencast-geometry.test.ts`
  (new) — both bar orientations, exact no-letterbox case, bar-click dropped.
- `crates/agentum-desktop/ui/src/components/browser-pane/AgentBrowserScreencastPane.tsx`
  — `toDevicePoint` now wraps `clientToDevicePoint` (contain-aware); **new
  first-frame viewport re-sync** effect: re-send `sendViewport()` once the first
  frame lands to force a re-capture at the pane aspect (the idle-page single-frame
  case → the "works only after remount" bug). Idempotent; NOT a timer poll.

**Gate:** `screencast-geometry.test` → **8/8**; `bun run build` → **green**.

**First-frame spike (architecture §5) — resolved to the UI-only path:** re-sending
the viewport after the first frame forces the relayout/re-capture; no server-side
`Page.startScreencast` re-arm was added. **QA-deferred (needs running app + CDP
page):** "no black bars on first open" + "clicks land" are `qa.sh`/installed-app
checks (Mateo). If QA shows the UI nudge insufficient on a fully-idle page, the
permitted fallback is a bounded server re-capture in `cdp_screencast.rs` (still no poll).

---
**All three features (F1 · F3 · F2) code-complete + unit-green → tester.**
Browser QA (F2/F3 live behaviors) is installed-app/`qa.sh`-deferred to Mateo.
