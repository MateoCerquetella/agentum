---
phase: 02-card-session-binding
plan: "05"
subsystem: dashboard-tui
tags: [dashboard, svelte, tui, ratatui, bound-session-panel, back-link, unbind, focus-card, pane-snapshot]
dependency_graph:
  requires: [02-02, 02-03]
  provides: [client-surfaces-phase2]
  affects: [dashboard/src/lib/api.ts, dashboard/src/lib/components/BoardItemDialog.svelte, dashboard/src/routes/sessions/[id]/+page.svelte, dashboard/src/routes/board/+page.svelte, crates/agentum/src/commands/terminal/app.rs, crates/agentum/src/commands/terminal/ui.rs, crates/agentum/src/commands/terminal/api.rs]
tech_stack:
  added: []
  patterns: [AbortController pane polling, Svelte 5 $effect lifecycle, ratatui palette-only chip, TUI guard-gated key handler]
key_files:
  created: []
  modified:
    - dashboard/src/lib/api.ts
    - dashboard/src/lib/components/BoardItemDialog.svelte
    - dashboard/src/routes/sessions/[id]/+page.svelte
    - dashboard/src/routes/board/+page.svelte
    - crates/agentum/src/commands/terminal/api.rs
    - crates/agentum/src/commands/terminal/app.rs
    - crates/agentum/src/commands/terminal/ui.rs
decisions:
  - "Hint strip rendered as overlay above status bar (not a layout slot) — avoids reflow of terminal pane; accepted ~1-row overlap with terminal content"
  - "session.card_id typed as number|null in TypeScript Session interface (not i64) — standard JS number covers the SQLite int8 range for card IDs"
  - "Non-null assertion (card!) used for back-link chip to satisfy TS strict mode inside {#if ... && card} block where svelte-check can't narrow"
  - "HintCardState placed before App struct in app.rs (consistent with other overlay-like structs)"
  - "get_board_item uses BoardItemSummary (id + title) not the full BoardItem type — TUI only needs the title for the hint strip"
metrics:
  duration: "~90 minutes"
  completed: "2026-05-22T19:30:55Z"
  tasks_completed: 4
  files_changed: 7
---

# Phase 02 Plan 05: Client Surfaces — Bound-Session Panel, Back-link Chip, TUI Hint Strip

One-liner: Full dashboard + TUI client-side exposure of Phase 2 session-card binding: pane polling panel, unbind, back-link chip, board focus handler, and TUI c-key hint strip with palette-only colors.

## What Was Built

All client-side surfaces for Phase 2 shipped in a single vertical slice across 4 tasks:

### Task 1 — api.ts + BoardItemDialog.svelte

**`dashboard/src/lib/api.ts`:**
- Added `PaneSnapshot` interface (`{ lines: string[]; captured_at: string }`)
- Added `getSessionPane(id, lines?, opts?: { signal?: AbortSignal })` — AbortSignal plumbed through existing `request<T>` fetch wrapper
- Added `getBoardItem(id)` and `getBoardItemOn(profileId, id)` — GET `/api/board/{id}`
- Added `card_id?: number | null` to `Session` interface (mirrors `agentum-core::Session.card_id`)

**`dashboard/src/lib/components/BoardItemDialog.svelte`:**
- Bound-session panel above comments section: eyebrow "BOUND SESSION" + `StatusPill`, 20-line pane tail polled every 2s with AbortController, Open-session deep link
- Polling lifecycle: pauses on `visibilityState === 'hidden'`, backs off after 3 errors, AbortController cancels in-flight fetch on dialog close
- Unbind button next to Open-session: PATCH `{ session_id: null }` with optimistic local update
- System comment styling: `.cmt-item.system .cmt-author { color: var(--fg-3) }` and `.cmt-item.system.crash { border-left: 2px solid var(--crash) }`

### Task 2 — sessions/[id] + board/?focus

**`dashboard/src/routes/sessions/[id]/+page.svelte`:**
- Added `card` and `parentGoal` state; fetched via `api.getBoardItem` in `reload()`
- Back-link chip in toolbar: `← Card #N (in "goal-title-truncated-to-40-chars")` or `← Card #N`; navigates via `goto('/board?focus={id}')` with `aria-label`
- Imports `BoardItem` type

**`dashboard/src/routes/board/+page.svelte`:**
- Imports `page` from `$app/state` and `goto` from `$app/navigation`
- `$effect` reads `page.url.searchParams.get('focus')`, scrolls `[data-card-id="${id}"]` into view, applies `focus-pulse` CSS animation, clears param via `goto('/board', { replaceState: true })`
- Each `<Ticket>` wrapped in `<div data-card-id={tk.id}>` for scroll targeting
- `@keyframes focus-pulse` + `:global([data-card-id].focus-pulse)` CSS animation

### Task 3 — TUI c-key + status bar chip + hint strip

**`crates/agentum/src/commands/terminal/api.rs`:**
- Added `BoardItemSummary` struct (`id: i64`, `title: Option<String>`)
- Added `Client::get_board_item(id: i64)` — GET `/api/board/{id}`, returns `BoardItemSummary`

**`crates/agentum/src/commands/terminal/app.rs`:**
- Added `HintCardState { card_id: i64, title: String }` struct (derives Clone, PartialEq, Eq, Debug)
- Added `hint_card: Option<HintCardState>` field to `App` (initialized `None`)
- `'c'` arm in `handle_key`: fires only when `Focus::Tree && key.modifiers.is_empty()` and `sess.card_id.is_some()`; toggles hint_card via `client.get_board_item()`; tracing::warn on fetch failure
- Esc arm clears `hint_card` when `Overlay::None` (peel-one-layer pattern)
- `s` key handler: unchanged (count verified before/after: 5)
- 4 unit tests in `hint_card_tests` module

**`crates/agentum/src/commands/terminal/ui.rs`:**
- `draw_status`: appends ` c card #N ` chip to `right` vec using `p.muted` / `p.chrome_bg` when `Focus::Tree` and `sess.card_id.is_some()`
- `draw` function: renders hint strip as overlay one row above status bar when `app.hint_card.is_some()`, using `p.fg` / `p.surface_bg`; no hardcoded `Color::*`

### Task 4 — Rebuild

```
npm run build --prefix dashboard   # ✓ exit 0
cargo build --release              # ✓ exit 0
target/release/agentum --version   # agentum 0.8.2
```

The embedded SPA reflects all Phase 2 dashboard changes.

## UI-SPEC Quality Bar — Demoably True

- [x] Bound-session panel visible in BoardItemDialog when card.session_id is set (eyebrow, StatusPill, pane tail, Open-session link)
- [x] Pane tail polls every 2s with AbortController; pauses on visibility-hidden; backs off after 3 errors
- [x] Unbind button PATCHes `{ session_id: null }` with optimistic clear and Unbinding… label
- [x] System comment rows styled muted; crashed variant has 2px --crash left border
- [x] Back-link chip in session topbar with `← Card #N (in "...")` and 40-char truncation
- [x] /board?focus=N scrolls + pulses (animationend cleanup) + clears URL param via replaceState
- [x] TUI status bar shows ` c card #N ` chip when session has card_id and Focus::Tree
- [x] Pressing `c` in Focus::Tree toggles one-cell hint strip with card title (palette-only colors)
- [x] `s` key unchanged (Stop session — 5 arms before and after)
- [x] No hardcoded `Color::*` in new ui.rs code
- [x] No `eprintln!` in app.rs or ui.rs
- [x] `npm run check --prefix dashboard` exits 0 (svelte-check + tsc strict)
- [x] `cargo clippy -p agentum --all-targets -- -D warnings` exits 0
- [x] `cargo test -p agentum --lib -- terminal::app::hint_card_tests` — 4 tests pass

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| 1 | `2e82dbd` | feat(02-05): add pane-snapshot client + bound-session panel + unbind + system-comment styling |
| 2a | `6a262e0` | feat(02-05): add back-link chip and board?focus handler |
| 3 | `c61b714` | feat(02-05): TUI c-key hint strip + status-bar card chip |
| 2b | `17f5aad` | feat(02-05): enhance back-link chip with card+goal fetch and board?focus clear |
| Build | (no commit — build artifacts gitignored) | npm run build + cargo build --release |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Task 2 acceptance criteria required full chip text**
- **Found during:** Task 2 verification
- **Issue:** Initial back-link chip implementation only showed `# {session.card_id}` — missing the `←` arrow, goto() call, title truncation, and aria-label required by the plan acceptance criteria
- **Fix:** Added `card` + `parentGoal` state, card fetch in `reload()`, full chip text with `← Card #N (in "...")` format, 40-char truncation, `goto()` onclick, `aria-label`
- **Files modified:** `dashboard/src/routes/sessions/[id]/+page.svelte`
- **Commit:** `17f5aad`

**2. [Rule 1 - Bug] Task 2 board page missing goto import and replaceState**
- **Found during:** Task 2 acceptance criteria verification
- **Issue:** `goto` was not imported; `replaceState: true` call was missing from the `$effect`
- **Fix:** Added `import { goto } from '$app/navigation'`; added `void goto('/board', { replaceState: true })` after scroll
- **Files modified:** `dashboard/src/routes/board/+page.svelte`
- **Commit:** `17f5aad`

**3. [Rule 3 - Block] svelte-check fails in worktree — missing node_modules**
- **Found during:** Task 1 verification
- **Issue:** Worktree at `.claude/worktrees/agent-a92568f9464048a4b/dashboard/` has no `node_modules/`; `svelte-kit` command not found
- **Fix:** Created symlink `dashboard/node_modules → /home/malloc/Developer/projects/agentum/dashboard/node_modules`; runs `svelte-check` from the worktree dashboard dir
- **Files modified:** None (worktree setup)

## Known Stubs

None — all data flows are wired to live API calls (getSessionPane, getBoardItem, patchBoardItemOn).

## Threat Flags

None — no new network endpoints introduced. All surfaces consume existing Phase 2 routes added in plans 02-02 and 02-03.

## Self-Check: PASSED

- [x] `dashboard/src/lib/api.ts` contains `getSessionPane`, `PaneSnapshot`, `getBoardItem`, `card_id`
- [x] `dashboard/src/lib/components/BoardItemDialog.svelte` contains `BOUND SESSION`, `Open session →`, `aria-live="polite"`, `setInterval(tick, 2000)`, `new AbortController()`, `paneController.signal`, `class:system=`, `class:crash=`, `unbindSession`, `patchBoardItemOn`
- [x] `dashboard/src/routes/sessions/[id]/+page.svelte` contains `back-link`, `← Card #`, `slice(0, 40)`, `aria-label`, `goto`
- [x] `dashboard/src/routes/board/+page.svelte` contains `searchParams.get('focus')`, `scrollIntoView`, `replaceState: true`, `data-card-id`
- [x] `crates/agentum/src/commands/terminal/app.rs` contains `HintCardState`, `hint_card`, `KeyCode::Char('c')` with Focus::Tree guard
- [x] `crates/agentum/src/commands/terminal/ui.rs` contains `card #`, `p.muted`
- [x] `dashboard/build/index.html` exists (build complete)
- [x] `target/release/agentum --version` exits 0
- [x] Commits 2e82dbd, 6a262e0, c61b714, 17f5aad present in git log
