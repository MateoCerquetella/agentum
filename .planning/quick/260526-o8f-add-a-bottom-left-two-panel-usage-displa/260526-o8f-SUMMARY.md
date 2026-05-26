---
quick_id: 260526-o8f
status: complete
outcome: success
files_changed:
  - crates/agentum/src/commands/terminal/ui.rs
commits:
  - d23cbb5: feat(tui-260526-o8f) add format helpers + draw_usage_panel for Usage widget
  - fa244ca: feat(tui-260526-o8f) splice Usage panel into bottom of tree column
requirements:
  - QUICK-260526-o8f
---

# 260526-o8f — Bottom-left Usage panel

## Outcome

New "Usage" widget renders in the bottom 10 rows of the TUI tree column showing per-tool aggregate (top) and per-session detail (bottom) for running sessions; tokens use k/M, cost `$X.XX`, ctx `NN%`, `—` for missing/invalid. Hidden in fullscreen and on viewports where the tree column is < 18 rows tall.

## Files changed

`crates/agentum/src/commands/terminal/ui.rs` (+258 / −1)
- L10: added `use std::collections::BTreeMap;`
- L28–L48: added `pub usage: Rect` field to `Areas` struct (doc-commented).
- L107 (fullscreen branch): added `usage: empty` to the early-return Areas literal.
- L196–L228 (normal branch): split `body[0]` vertically into `(tree_rect, usage_rect)` with a 10-row Length constraint when `tree_full.height >= 18`, else zero-sized usage. Replaced `tree: body[0]` with `tree: tree_rect` and added `usage: usage_rect` to the final Areas literal.
- L315–L317 (in `draw()`): added the `draw_usage_panel(...)` call after the tree-draw call, gated on both width and height.
- L3240–L3457 (append, after `overlay_box_with_title_style`): added
  - `pub(super) fn format_tokens(t: Option<i64>) -> String`
  - `pub(super) fn format_cost(c: Option<f64>) -> String`
  - `pub(super) fn format_ctx(p: Option<i32>) -> String`
  - `fn truncate_pad(s: &str, n: usize) -> String`
  - `pub(super) fn draw_usage_panel(f, area, app, p)`
  - `#[cfg(test)] mod tests` with the 3 required `format_*_variants` tests.

No other file touched.

## Commands run

| Command | Result |
|---|---|
| `cargo build -p agentum` (after Task 1) | OK (dead-code warns expected — wired in Task 2) |
| `cargo test -p agentum --lib commands::terminal::ui::tests` | 3 passed |
| `cargo build -p agentum` (after Task 2) | OK, no warnings |
| `cargo clippy -p agentum --all-targets -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean (after one auto-rustfmt pass on the bottom-half Span block) |
| `cargo test -p agentum --lib` | 105 passed, 0 failed |
| `cargo build --release -p agentum` | OK, release profile finished |

## Deviations from PLAN.md

None. The plan's interface note about the real `i64`/`i32` Session types (vs. the brief's `u64`/`u8`) was honoured directly. One cosmetic rustfmt pass after Task 2 — rustfmt reformatted a single `Span::styled(...)` call from 1-line to 3-line; no logic change.

## Visual verification (manual — not part of CI)

To eyeball the rendering:
```sh
cargo build --release -p agentum   # already run above
./target/release/agentum serve     # in one terminal
./target/release/agentum terminal  # in another, with 2+ running sessions
```

Expected:
- Bottom 10 rows of the left sidebar carries a `┌ Usage ┐` block.
- Top half: "Agents" header in accent color; rows of `claude   2 sess   42.0k`-style aggregates, sorted by session count desc then tool name asc.
- Bottom half: "Tasks" header; rows of `mysess     42%   12.5k   $0.34`-style detail, sorted by tokens desc.
- With zero running sessions: centered muted "No active agents" line.
- Resize the terminal so the left column drops below 18 rows: the Usage panel disappears and the session list reclaims the rows.
- Toggle fullscreen (the existing keybind): Usage panel hidden along with the rest of chrome.

## Self-Check: PASSED

- `crates/agentum/src/commands/terminal/ui.rs` — FOUND (mtime recent, format helpers + draw fn + tests present)
- commit `d23cbb5` — FOUND in `git log --oneline`
- commit `fa244ca` — FOUND in `git log --oneline`
- 3 unit tests pass under `commands::terminal::ui::tests`
- release build of `agentum` succeeds with no warnings
- no file outside `crates/agentum/src/commands/terminal/ui.rs` modified by these commits (verified via `git diff de891bf..HEAD --stat`)
