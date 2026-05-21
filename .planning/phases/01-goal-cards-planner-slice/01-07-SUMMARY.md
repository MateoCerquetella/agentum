---
phase: 01-goal-cards-planner-slice
plan: "07"
subsystem: tui
tags: [tui, ratatui, overlay, goal-submission, palette]
dependency_graph:
  requires: [01-03]
  provides: [tui-goal-overlay, tui-parent-cue]
  affects: []
tech_stack:
  added: []
  patterns: [Overlay::Goal, GoalForm state machine, draw_overlay_goal, ApiClient::submit_goal]
key_files:
  created: []
  modified:
    - crates/agentum/src/commands/terminal/api.rs
    - crates/agentum/src/commands/terminal/app.rs
    - crates/agentum/src/commands/terminal/ui.rs
decisions:
  - "GoalForm boxed inside Overlay::Goal to keep the enum size flat — mirrors the existing NewSession overlay pattern"
  - "Ctrl-Enter submits, Esc cancels — matches the UI-SPEC §6 revision (commit aa1c494) that aligned with terminal idioms rather than Cmd-Enter"
  - "Palette-only colors via the existing palette.rs; no Color::Rgb literals in the new code"
  - "G-keybinding gated on Board view + no overlay open + no input focus — prevents shadowing in-progress text entry"
  - "Status-bar copy 'Goal submitted ◆ AG-XXX' matches UI-SPEC §6 verbatim (diamond glyph + AG-key)"
  - "o-to-jump-to-parent only fires when focus is on a child card with parent_goal_id set; no-op elsewhere"
metrics:
  completed: "2026-05-21"
  tasks: 2
  files_changed: 3
---

# Phase 01 Plan 07: TUI Goal Overlay Summary

The terminal half of the goal slice: an `Overlay::Goal` reachable via `G` on the Board view (mirrors `Overlay::NewSession`), an `ApiClient::submit_goal(text)` HTTP method, parent-cue rendering on child cards, the `lbl=goal` styling, and the `o`-to-jump-to-parent keybinding. Honours the UI-SPEC contract verbatim — palette-only colours, Ctrl-Enter to submit, Esc to cancel, status-bar copy matching §6.

## What Was Built

### `ApiClient::submit_goal` (api.rs)

`pub async fn submit_goal(&self, text: &str) -> Result<GoalCreateResp, ClientError>` posts to `/api/board/goals` via the same `request_with_token<T>` pattern used for `create_session`. Returns the new `{ id, key }` so the caller can show the AG-key in the status bar.

### `Overlay::Goal` + `GoalForm` (app.rs)

```rust
pub enum Overlay {
    ...
    Goal(Box<GoalForm>),
}

pub struct GoalForm {
    pub text: String,                  // multi-line buffer
    pub profile: String,               // which endpoint to submit to
    pub state: GoalFormState,          // Composing | Submitting | Submitted | Error
    pub submitted_key: Option<String>, // AG-key after success
    pub error: Option<String>,         // user-facing error
}
```

`GoalForm` is boxed inside the `Overlay` enum to keep the variant flat — same trick used for `NewSession`. State transitions:

- `Composing` (default) — user types into `text`.
- `Submitting` (Ctrl-Enter) — overlay locked, HTTP in flight.
- `Submitted` (success) — status bar shows AG-key, overlay closes on next key.
- `Error` (failure) — error shown in overlay; Ctrl-Enter retries, Esc cancels.

### Key Handler (`handle_overlay_goal_key` in app.rs)

| Key | Action |
|-----|--------|
| Esc | Discard + close overlay |
| Ctrl-Enter | Submit if `text` non-empty; otherwise no-op |
| Enter (no ctrl) | Insert newline into `text` |
| Backspace | Delete last char |
| Char | Append to `text` |

The `G` keybinding (in `handle_key`) opens `Overlay::Goal` when:
1. The active view is `View::Board` AND
2. No overlay is currently open AND
3. No input field is focused.

Otherwise `G` falls through to its existing meaning (Goto, etc.).

### Render Path (`draw_overlay_goal` in ui.rs)

The overlay renders as a centred modal:

- **Title row**: "New goal" with the active profile chip on the right (`@local`, `@vps`, etc. — mirrors the Profiles overlay).
- **Body**: multi-line `Paragraph` of `form.text` with a blinking cursor.
- **Footer hint row**: "Ctrl-Enter submit · Esc cancel" — palette-driven dim style.
- **State indicator**: spinner during `Submitting`, AG-key + green dot on `Submitted`, red error message on `Error`.

All colours come from `Palette::overlay_fg`, `Palette::overlay_bg`, `Palette::overlay_border`, `Palette::accent`, `Palette::error` — no hardcoded RGB.

### Parent-Cue + `lbl=goal` Styling

In `draw_board_panel`, when a ticket has `parent_goal_id` set, the title row gets a `↑ AG-XXX` cue rendered in `Palette::dim` after the ticket key. When `lbl == "goal"`, the lbl badge uses `Palette::goal_accent` instead of the default `Palette::lbl_default`. UI-SPEC §3.2 / §3.3 conformance.

### `o`-to-Jump-to-Parent

In `handle_board_panel_key`, an `o` keypress on a child ticket reads `app.focused_ticket().parent_goal_id`, finds the parent ticket by id in `app.board`, sets focus to it, and emits a brief flash on the parent row. No-op when:
- Focus isn't on a ticket
- The focused ticket has no `parent_goal_id`
- The parent is not currently in `app.board` (e.g. filtered out)

### Status-Bar Copy

On successful submission, the status bar transitions to:

```
Goal submitted ◆ AG-7K9X
```

The `◆` is the diamond glyph from UI-SPEC §6.4. The AG-key comes from `submit_goal`'s response. After 3 s the status bar reverts to its default content.

## Tests

5 new `#[test]` and `#[tokio::test]` cases in `agentum::commands::terminal::tests`:

- `goal_overlay_open_via_g_key_from_board_view` — G from `View::Board` with no overlay opens `Overlay::Goal`.
- `goal_overlay_g_no_op_when_overlay_already_open` — G falls through when an overlay is already open.
- `goal_overlay_should_submit_true_for_nonempty_text` — predicate test.
- `goal_overlay_should_submit_false_for_whitespace_only` — leading/trailing whitespace alone doesn't submit.
- `goal_overlay_handle_enter_inserts_newline` — Enter (no ctrl) inserts `\n` into `text`.

Plus the 5 RED tests from commit `9a75f84` that drove the GREEN implementation.

## Deviations from Plan

**Orchestrator finished the work after the parallel executor agent was halted mid-clippy.** The executor agent had committed `9a75f84` (the RED phase with failing tests) and left the GREEN implementation uncommitted in main's working tree while iterating on a clippy `needless_return` lint on the `KeyCode::Esc` arm of the overlay key handler. The orchestrator:

1. Verified the implementation compiled + passed all 17 agentum binary tests (including the 5 RED tests that now go GREEN).
2. Fixed the clippy `needless_return` warning by removing the explicit `return;` from the `KeyCode::Esc` arm (one-line edit).
3. Committed the rest as `feat(01-07): Overlay::Goal + submit_goal client + render — GREEN` (188b501).

This is the consequence of Claude Code's `isolation="worktree"` failing to actually isolate the agent — see the Wave 3 incident note for the broader context.

## Forward References (v2 Deferred Work)

- **Goal editing**: v1 only supports create. Edit/delete from the TUI is deferred — for now the user goes through the dashboard or `agentum board add-card --parent-goal …`.
- **Child auto-attach from the overlay**: typing a child card directly from the overlay (rather than waiting for the planner) is deferred to v2. v1 stops at "goal submitted, planner runs, children appear via the WS event stream".
- **Visual goal-grouping in the board panel**: child cards are currently rendered inline with the rest of the column; v2 will indent or visually group them under their parent.

## Self-Check: PASSED
