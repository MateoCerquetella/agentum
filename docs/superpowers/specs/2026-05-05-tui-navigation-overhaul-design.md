# TUI Navigation Overhaul

Date: 2026-05-05
Scope: `crates/agentum/src/commands/terminal/`

## Why

The TUI's pane focus model is hostile:

- Once focus is inside the terminal pane, the only way out is `Ctrl-G` (release) or
  `Ctrl-]/Ctrl-[` (cycle). The bottom Input pane is unreachable without a two-step
  dance (`Ctrl-G` → `i`/`3`).
- The Input pane itself is redundant — the terminal pane is already an interactive
  PTY, so typing in the terminal is the natural way to interact.
- Top and bottom chrome bars duplicate information (workdir, theme chip, `Ctrl-P
  palette` hint).
- Lazygit silently fails to start when the selected session's `workdir` doesn't
  exist locally (common when connected to a remote `agentum serve`).
- The keyboard map has accumulated alternates (`Ctrl-]`/`Ctrl-[`, `F5`/`F6`,
  `Tab`/`Shift-Tab`, numeric jumps) without a clear primary path.

## Changes

### 1. Drop the Input pane

- Remove `Focus::Input`, `App::input`, `App::term_in`-related wiring is unaffected,
  but the `Esc`/`Tab`/`Char` handlers for `Focus::Input` and the `client.send_text`
  call site they drove are deleted.
- `compute_layout` no longer reserves the bottom 3 rows; the right column is
  100% terminal area, with the lazygit split (when open) carved out of that.
- The terminal gains those 3 rows automatically — that's the "resize" ask.

### 2. Lazygit reliability

In `toggle_lazygit`:

- Resolve `cwd`: prefer the selected session's `workdir`, but if it doesn't exist
  or isn't a directory locally, fall back to `std::env::current_dir()`.
- Surface a clear status message when falling back: `"lazygit: session workdir not
  local — opened in <local cwd>"`.
- On spawn failure, set a sticky `status_msg` and bump `error_count` (already
  done) — additionally retain the failure reason in `status_msg` rather than
  letting it get clobbered by subsequent transient messages.

### 3. Keyboard shortcuts — simplified

**Universal** (work from any focus, including inside Term/Lazygit PTY):

| Key | Action |
|---|---|
| `Ctrl-]` | Next panel (Tree → Term → Lazygit → Tree) |
| `Ctrl-[` | Previous panel |
| `Ctrl-1`..`Ctrl-9` | Jump to Nth project group in the tree (focus Tree, expand, select first session in that group) |
| `Ctrl-P` / `Ctrl-K` | Command palette |
| `Ctrl-Q` | Quit |

**Tree-only**:

- `j`/`k`/`h`/`l` and arrows, `Enter`
- `n` (new), `u` (up), `s` (stop), `K` (kill), `D` (delete), `r` (refresh)
- `g` toggle lazygit, `G` lazygit cheatsheet
- `T` cycle theme, `?` help, `q` quit
- `Tab`/`Shift-Tab` cycle panels (alias for `Ctrl-]`/`Ctrl-[`)
- Plain `1`/`2`/`3` jump to panel (Tree/Term/Lazygit)

**Inside Term/Lazygit**:

- Every key except the universal escapes above is forwarded raw to the PTY.
- `Esc`, `Tab`, `Ctrl-C`, etc. all reach the running process.

**Removed**: `Ctrl-G`, `F5`/`F6`, `i` (input). The current panel-cycle alternates
(`Ctrl-]`/`Ctrl-[`) become primary; `Ctrl-G` and the function keys are gone.

`Ctrl-1`..`Ctrl-9` semantics: numbers index the tree's project (workdir) groups
in display order. Pressing `Ctrl-2` focuses the tree, moves the cursor to the
second project group's row, and expands the group. **It does NOT auto-select a
session** — the user navigates with arrows and presses `Enter` to pick one.
If fewer than N groups exist, the keypress is a no-op with a status message.

**`Enter` on a session leaf**: in addition to its current behavior (select +
start streaming), focus moves to the terminal pane. Pressing `Enter` on a group
row toggles expansion (current behavior, unchanged).

### 4. UI deduplication

**Top bar** (`draw_title`) becomes:

```
agentum · <session-name>     <theme-chip>
```

Drop the workdir (it's in the bottom bar) and the `Ctrl-P palette` hint (also in
the bottom bar).

**Bottom bar** (`draw_status`) keeps:

```
<workdir> · <tool/model> · <conn> · <errors> · [g lazygit | lazygit] · <status_msg> · Ctrl-P palette · ? help
```

Drop the theme chip from the bottom (it's now top-only).

**Panel border titles**:

- Tree: `" 1 sessions "` (unchanged)
- Terminal (focused): `" 2 terminal · Ctrl-] next · Ctrl-1 first project "`
- Terminal (unfocused): `" 2 terminal · Ctrl-] focus "`
- Lazygit (focused): `" 3 lazygit · Ctrl-] next · g close "`
- Lazygit (unfocused): `" 3 lazygit · Ctrl-] focus · g close "`

### 5. Help / cheatsheet / palette text

Update all places that document keys:

- `draw_help_overlay` (ui.rs)
- `palette_catalog` (app.rs) — palette action labels
- Any `status_msg` strings referring to dropped keys

## Files touched

- `crates/agentum/src/commands/terminal/app.rs` — focus model, key handler,
  toggle_lazygit, palette catalog, NewSession form fields untouched
- `crates/agentum/src/commands/terminal/ui.rs` — layout, draw_title,
  draw_status, draw_terminal/lazygit titles, draw_help_overlay, drop draw_input
- `crates/agentum/src/commands/terminal/palette.rs` — drop "focus input"
  action if present, update labels referring to old keys

## Out of scope

- Multi-line message composer overlay (could replace the deleted Input as a
  `Ctrl-Enter` modal later — not now).
- Lazygit-over-WebSocket so it works against remote workdirs (large; for now
  we just fail clearly and fall back to local cwd).
- Theme/palette redesign — only the chrome dedup, no color changes.

## Risks

- `Ctrl-1`..`Ctrl-9` may conflict with shells/apps inside the terminal pane that
  use those bindings. Acceptable trade — they map to common terminal-multiplexer
  conventions and the user explicitly asked for "Ctrl+N for the Nth project".
- Removing `Ctrl-G` may break muscle memory for existing users — documented in
  CHANGELOG, surfaced in `?` help.
