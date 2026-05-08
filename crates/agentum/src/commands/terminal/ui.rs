//! Pure draw functions for the terminal dashboard. No mutation, no IO.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use tui_term::widget::PseudoTerminal;

use std::time::SystemTime;

use agentum_core::Status as SessionStatus;
use agentum_core::transcript::{AgentTaskState, TaskStatus, TodoStatus};

use super::app::{
    AddProfileField, AddProfileForm, App, ConnState, DirPickerState, ErrorEntry, Focus,
    NewSessionField, NewSessionForm, NotifKind, Notification, Overlay, PendingAction,
    ProfilesOverlay, RenameState, Row, SettingsRow, SettingsState, Side, palette_catalog,
    status_dot,
};
use super::extensions::{self, Extension, LAZYGIT};
use super::iometer::{fmt_bytes, fmt_rate};
use super::prefs::{Prefs, SoundKind, StatusChip};
use super::theme::Palette;

#[derive(Clone, Copy)]
pub struct Areas {
    pub title: Rect,
    pub tree: Rect,
    /// Primary (left) terminal pane. Always present.
    pub terminal: Rect,
    /// Right terminal pane in a horizontal split. `Some` when the user
    /// has opened a split with Ctrl-\\. When set, `terminal` is the
    /// left half and this is the right.
    pub terminal_right: Option<Rect>,
    pub lazygit: Option<Rect>,
    /// Per-agent plan / todos / background-tasks panel pinned to the
    /// right edge. `Some` when the terminal is wide enough and the
    /// user hasn't toggled it off (Ctrl-T). Drawn for the currently
    /// selected agent only.
    pub agent_tasks: Option<Rect>,
    pub status: Rect,
}

/// Minimum total terminal width before the right panel is allowed.
/// Below this we hide it entirely so the terminal pane keeps enough
/// horizontal space to render embedded TUIs comfortably. 110 cols is
/// roughly "wide enough that the user opted into a wide terminal."
const RIGHT_PANEL_MIN_TOTAL_WIDTH: u16 = 110;
/// Width of the right panel when shown. 34 fits a Plan/Todos/Tasks
/// stack with truncation but doesn't overwhelm narrow setups.
const RIGHT_PANEL_WIDTH: u16 = 34;
/// Hard floor for the lazygit pane width when it gets a dedicated outer
/// column. Anything narrower than this and lazygit's own panels start
/// truncating headers, so we fall back to the in-pane split.
pub const LAZYGIT_MIN_WIDTH: u16 = 40;
/// Hard ceiling so the user can't accidentally crush the terminal pane
/// below its 20-col floor by holding the grow key.
pub const LAZYGIT_MAX_WIDTH: u16 = 160;
/// Clamp range for the term-split percentage. 25 / 75 keeps either half
/// above ~30 cols on a 110-col viewport.
pub const TERM_SPLIT_MIN_PCT: u16 = 25;
pub const TERM_SPLIT_MAX_PCT: u16 = 75;
/// Step size per `Ctrl-Shift-←/→` press.
pub const TERM_SPLIT_STEP: u16 = 5;

pub fn compute_layout(
    area: Rect,
    lazygit_open: bool,
    fullscreen: bool,
    tree_width: u16,
    sidebar_hidden: bool,
    split_open: bool,
    right_panel_visible: bool,
    lazygit_width: u16,
    term_split_pct: u16,
) -> Areas {
    // Right panel is suppressed in fullscreen and on terminals too
    // narrow to host it without crushing the terminal area. It stays
    // visible alongside lazygit so the agent's plan/todos/tasks remain
    // in view while the user works in git.
    let show_right = right_panel_visible
        && !fullscreen
        && area.width >= RIGHT_PANEL_MIN_TOTAL_WIDTH;

    // Fullscreen: drop the title row, tree column, and status row so the
    // active panes consume every available cell. The empty Rects keep the
    // draw_* helpers no-op (they short-circuit on `area.width == 0`).
    if fullscreen {
        let (terminal_rect, lazygit_rect) =
            split_main(area, lazygit_open, lazygit_width);
        let (term_left, term_right) = split_terminal(terminal_rect, split_open, term_split_pct);
        let empty = Rect {
            x: area.x,
            y: area.y,
            width: 0,
            height: 0,
        };
        return Areas {
            title: empty,
            tree: empty,
            terminal: term_left,
            terminal_right: term_right,
            lazygit: lazygit_rect,
            agent_tasks: None,
            status: empty,
        };
    }

    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    // Clamp tree width so the terminal pane always has at least 20 cols
    // even on narrow terminals where the user widened the sidebar.
    // Sidebar-hidden (Ctrl-B / Ctrl-K B) collapses the tree column to 0
    // while leaving title and status bars in place.
    let tw = if sidebar_hidden {
        0
    } else {
        let max_tree = v[1].width.saturating_sub(20);
        tree_width.min(max_tree)
    };

    // Lazygit gets its own outer column on the far right when it's open
    // AND we have room to host a 20-col terminal next to it. On narrow
    // viewports we fall back to the in-pane split (handled below by
    // `split_main` receiving `lw == 0` and `lazygit_open == true`).
    let lw_target = if lazygit_open {
        lazygit_width.clamp(LAZYGIT_MIN_WIDTH, LAZYGIT_MAX_WIDTH)
    } else {
        0
    };
    // Try to fit tree + 20-col term + lazygit. If lazygit can't fit at
    // min width, leave it inline. The agent_tasks panel gets dropped
    // first (handled below in the `rw` calc) — keeping lazygit pinned
    // right is the explicit user preference.
    let lw = if lw_target == 0
        || v[1].width.saturating_sub(tw).saturating_sub(lw_target) < 20
    {
        // Not enough room for a dedicated column: signal the in-pane
        // split fallback by leaving lw at zero.
        0
    } else {
        lw_target
    };

    // Reserve the right column when shown. Falls back to 0 if doing so
    // would push the terminal pane below its 20-col floor — counting
    // both the lazygit outer column and the right panel together.
    let rw = if show_right
        && v[1]
            .width
            .saturating_sub(tw)
            .saturating_sub(lw)
            .saturating_sub(RIGHT_PANEL_WIDTH)
            >= 20
    {
        RIGHT_PANEL_WIDTH
    } else {
        0
    };
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(tw),
            Constraint::Min(20),
            Constraint::Length(rw),
            Constraint::Length(lw),
        ])
        .split(v[1]);

    // When `lw > 0` the outer column owns the lazygit Rect; otherwise
    // we let `split_main` carve it out of the terminal area (the legacy
    // in-pane split for narrow viewports).
    let (terminal_rect, lazygit_rect) = if lw > 0 {
        (body[1], Some(body[3]))
    } else {
        split_main(body[1], lazygit_open, lazygit_width)
    };
    let (term_left, term_right) = split_terminal(terminal_rect, split_open, term_split_pct);
    let agent_tasks_rect = if rw > 0 { Some(body[2]) } else { None };

    Areas {
        title: v[0],
        tree: body[0],
        terminal: term_left,
        terminal_right: term_right,
        lazygit: lazygit_rect,
        agent_tasks: agent_tasks_rect,
        status: v[2],
    }
}

fn split_main(area: Rect, lazygit_open: bool, lazygit_width: u16) -> (Rect, Option<Rect>) {
    if !lazygit_open {
        return (area, None);
    }
    if area.width >= 100 {
        // Honour the user's chosen width as a fixed split when there's
        // room; clamp so the terminal half keeps at least 40 cols. This
        // makes the resize key affect the inline split too, not just
        // the dedicated outer column.
        let lw = lazygit_width
            .clamp(LAZYGIT_MIN_WIDTH, LAZYGIT_MAX_WIDTH)
            .min(area.width.saturating_sub(40));
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Length(lw)])
            .split(area);
        (cols[0], Some(cols[1]))
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);
        (rows[0], Some(rows[1]))
    }
}

/// Split the terminal area into a left/right pair when the user has
/// opened a split (Ctrl-\\). Wide terminals split horizontally (50/50);
/// narrow ones stack vertically so each pane keeps a usable width. We
/// use 80 columns as the cutoff because below that horizontal halves
/// pinch every embedded TUI under the 40-col floor most expect.
fn split_terminal(
    area: Rect,
    split_open: bool,
    term_split_pct: u16,
) -> (Rect, Option<Rect>) {
    if !split_open {
        return (area, None);
    }
    let pct = term_split_pct.clamp(TERM_SPLIT_MIN_PCT, TERM_SPLIT_MAX_PCT);
    if area.width >= 80 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(pct), Constraint::Percentage(100 - pct)])
            .split(area);
        (cols[0], Some(cols[1]))
    } else {
        // Narrow viewports stack vertically — keep 50/50 since up/down
        // would collide with tree navigation if we wired a resize.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        (rows[0], Some(rows[1]))
    }
}

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let areas = compute_layout(
        f.area(),
        app.lazygit_open(),
        app.fullscreen,
        app.tree_width,
        app.sidebar_hidden,
        app.split_open(),
        app.right_panel_visible,
        app.lazygit_width,
        app.term_split_pct,
    );
    let p = &app.theme.palette;

    // Paint the body background across the entire frame so the void around
    // panels takes the theme colour, not the host terminal's default.
    let body = Block::default().style(Style::default().bg(p.body_bg).fg(p.fg));
    f.render_widget(body, f.area());

    if areas.title.height > 0 {
        draw_title(f, areas.title, app, p);
    }
    if areas.tree.width > 0 {
        draw_tree(f, areas.tree, app, p);
    }
    draw_terminal(f, areas.terminal, app, p);
    if let Some(right_area) = areas.terminal_right {
        draw_terminal_right(f, right_area, app, p);
    }
    if let Some(lg_area) = areas.lazygit {
        draw_lazygit(f, lg_area, app, p);
    }
    if let Some(rp_area) = areas.agent_tasks {
        draw_agent_tasks_panel(f, rp_area, app, p);
    }
    if areas.status.height > 0 {
        draw_status(f, areas.status, app, p);
    }

    // Bottom-left toast stack — overlays whatever was just drawn so it
    // covers chrome / panes but is itself covered by modal overlays
    // below. Skipped in fullscreen because there's no status bar to
    // anchor above and a stack of bordered blocks over a fullscreen
    // pane would obscure the pane content the user opted into.
    if !app.fullscreen && !app.notifications.is_empty() {
        draw_notifications(f, f.area(), app, p);
    }

    let show_reconnect = app.was_connected && app.conn != ConnState::Connected;
    if show_reconnect {
        draw_reconnect_overlay(f, f.area(), app, p);
    }

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => draw_help_overlay(f, f.area(), app.lazygit_open(), p),
        Overlay::LazygitCheats => draw_cheatsheet_overlay(f, f.area(), &LAZYGIT, p),
        Overlay::LazygitInstall => draw_install_overlay(f, f.area(), &LAZYGIT, p),
        Overlay::Palette => draw_palette_overlay(f, f.area(), app, p),
        Overlay::Errors => draw_errors_overlay(f, f.area(), app, p),
        Overlay::NewSession(form) => {
            // Pass `app.tool_available(...)` through as a precomputed
            // bool so the renderer can show "(not installed)" next to
            // the Tool field without taking a borrow on `app` itself
            // (the overlay's `form` is already a borrow against `app`).
            let tool_unavailable = !app.tool_available(form.tool.trim());
            draw_new_session_overlay(f, f.area(), form, tool_unavailable, p)
        }
        Overlay::Confirm(action) => draw_confirm_overlay(f, f.area(), action, p),
        Overlay::Settings(state) => draw_settings_overlay(f, f.area(), state, &app.prefs, p),
        Overlay::Rename(state) => draw_rename_overlay(f, f.area(), state, p),
        Overlay::Profiles(state) => {
            draw_profiles_overlay(f, f.area(), state, app.active_profile.as_deref(), p)
        }
    }
}

fn panel_block<'a>(title: &'a str, focused: bool, p: &Palette) -> Block<'a> {
    Block::default()
        .title(Span::styled(
            title.to_string(),
            Style::default()
                .fg(if focused { p.accent } else { p.fg })
                .add_modifier(if focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ))
        .borders(Borders::ALL)
        .border_style(border_style(focused, p))
        .style(Style::default().bg(p.panel_bg).fg(p.fg))
}

fn draw_title(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    // Top bar is intentionally minimal: app name + active session on the
    // left, an error chip on the right when the log isn't empty. The
    // chip used to live in the bottom status bar; pulling it up here
    // keeps the bottom row reserved for non-error feedback while still
    // surfacing "something failed — press !" in the user's eye-line.
    // Endpoint suffix surfaces the active profile so users juggling
    // multiple agentum servers (local + VPS) can see which one drives
    // the current pane without opening Ctrl-O. Hidden when no profile
    // is active (loopback, ad-hoc `--api`) to keep the bar tidy.
    let endpoint_suffix = match app.active_profile.as_deref() {
        Some(name) => format!(" · @{name}"),
        None => String::new(),
    };
    let title = match app.selected_session() {
        Some(s) => format!(" agentum · {}{endpoint_suffix} ", s.name),
        None => format!(" agentum · no session selected{endpoint_suffix} "),
    };
    let title_span = Span::styled(
        title,
        Style::default()
            .fg(p.fg_strong)
            .bg(p.body_bg)
            .add_modifier(Modifier::BOLD),
    );

    // Right-aligned error chip. Padded with a leading column of body bg
    // so the chip doesn't smear into the (possibly truncated) title when
    // the terminal is narrow.
    if app.error_count == 0 {
        let para = Paragraph::new(Line::from(vec![title_span]))
            .style(Style::default().bg(p.body_bg));
        f.render_widget(para, area);
        return;
    }

    let chip_text = format!(
        " ⚠ {} error{} · press ! ",
        app.error_count,
        if app.error_count == 1 { "" } else { "s" }
    );
    let chip = Span::styled(
        chip_text.clone(),
        Style::default()
            .fg(p.error)
            .bg(p.chrome_bg)
            .add_modifier(Modifier::BOLD),
    );

    let chip_w = chip_text.chars().count() as u16;
    let total = area.width;
    let title_w = total.saturating_sub(chip_w).saturating_sub(1);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(title_w),
            Constraint::Length(1),
            Constraint::Length(chip_w),
        ])
        .split(area);

    let title_para = Paragraph::new(Line::from(vec![title_span]))
        .style(Style::default().bg(p.body_bg));
    f.render_widget(title_para, cols[0]);
    let gap = Paragraph::new("").style(Style::default().bg(p.body_bg));
    f.render_widget(gap, cols[1]);
    let chip_para = Paragraph::new(Line::from(vec![chip]))
        .style(Style::default().bg(p.body_bg))
        .alignment(ratatui::layout::Alignment::Right);
    f.render_widget(chip_para, cols[2]);
}

fn draw_tree(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let focused = app.focus == Focus::Tree;
    // Title shows the active filter so the user always sees what's
    // narrowing the list. While filter-input mode is active we append a
    // trailing `_` to hint at the live cursor.
    let filter = app.tree.filter_str();
    let count = app.sessions.len();
    let noun = if count == 1 { "session" } else { "sessions" };
    let title = if app.filter_input_active {
        format!(" {count} {noun} · ⌕{filter}_ ")
    } else if !filter.is_empty() {
        format!(" {count} {noun} · ⌕{filter} ")
    } else {
        format!(" {count} {noun} ")
    };
    let block = panel_block(&title, focused, p);

    let mut items: Vec<ListItem> = Vec::new();
    let cursor = app.tree.cursor;
    for (i, row) in app.tree.rows().iter().enumerate() {
        let is_cursor = i == cursor;
        items.push(render_tree_row(app, *row, is_cursor, focused, p));
    }

    if items.is_empty() {
        let hint = if !filter.is_empty() {
            format!("  (no matches for ⌕{filter})")
        } else {
            "  (no sessions — `agentum new …`)".to_string()
        };
        items.push(ListItem::new(Line::from(Span::styled(
            hint,
            Style::default().fg(p.muted),
        ))));
    }

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_tree_row(
    app: &App,
    row: Row,
    is_cursor: bool,
    panel_focused: bool,
    p: &Palette,
) -> ListItem<'static> {
    let row_style = if is_cursor {
        Style::default()
            .bg(p.cursor_bg)
            .fg(if panel_focused { p.cursor_fg } else { p.fg })
    } else {
        Style::default().bg(p.panel_bg).fg(p.fg)
    };
    match row {
        Row::Group(gi) => {
            let g = &app.tree.groups[gi];
            let arrow = if g.expanded { "▾" } else { "▸" };
            // Show the project name (basename) rather than the full path —
            // the full workdir is still visible in the title bar / status.
            let label = super::app::group_label(&g.workdir);
            ListItem::new(Line::from(vec![
                Span::raw(format!(" {arrow} ")),
                Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
            ]))
            .style(row_style)
        }
        Row::Leaf { group, leaf } => {
            let id = app.tree.groups[group].sessions[leaf];
            let session = app.sessions.iter().find(|s| s.id == id);
            let (name, dot, dot_color, tool_label) = match session {
                Some(s) => {
                    // Priority: Crashed > Awaiting > Idle > underlying
                    // status. A dead pane should never look like it's
                    // just waiting; a pending prompt overrides the
                    // green/idle dot so attention is unmissable; a
                    // sleeping agent reads as muted `◌` instead of
                    // green `●` so "working" and "idle at prompt" are
                    // visually distinct without a 2-cell emoji that
                    // would shift later spans.
                    let (dot, color) = if s.status == agentum_core::Status::Crashed {
                        status_dot(s.status)
                    } else if app.awaiting_input.contains(&s.id) {
                        ("▲", p.warning)
                    } else if app.idle.contains(&s.id) {
                        // accent_alt instead of muted — muted is the same
                        // dim grey used for the placeholder `—` and the
                        // tool/model label, so a sleeping agent
                        // disappeared into the background. accent_alt is
                        // distinct from green (working), yellow
                        // (awaiting), and red (crashed).
                        ("◌", p.accent_alt)
                    } else {
                        status_dot(s.status)
                    };
                    let tool = match s.model.as_deref() {
                        Some(m) => format!("{}/{}", s.tool, m),
                        None => s.tool.clone(),
                    };
                    (s.name.clone(), dot, color, tool)
                }
                None => ("?".into(), "?", p.error, "".into()),
            };
            let mut spans = vec![
                Span::raw("    "),
                Span::raw(format!("{:<14}", truncate(&name, 14))),
                Span::raw(" "),
                Span::styled(dot, Style::default().fg(dot_color)),
                Span::raw(" "),
                Span::styled(tool_label, Style::default().fg(p.muted)),
            ];
            if is_cursor {
                spans[1].style = Style::default().add_modifier(Modifier::BOLD);
            }
            ListItem::new(Line::from(spans)).style(row_style)
        }
    }
}

fn draw_terminal(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let focused = app.focus == Focus::Term;
    // When a split is open, brand the left pane explicitly so the user
    // knows which side keystrokes go to.
    let base = match (focused, app.split_open()) {
        (true, true) => " 2 left · Ctrl-E ↔ tree · type freely ",
        (false, true) => " 2 left · 2 / Ctrl-E focus ",
        (true, false) => " 2 terminal · Ctrl-E ↔ tree · type freely ",
        (false, false) => " 2 terminal · 2 / Ctrl-E focus ",
    };
    // Append a scrollback badge whenever the user has wheeled / Shift-PgUp
    // away from live output, so it's visually obvious why fresh bytes
    // aren't appearing. Any keystroke into the pane snaps back.
    let title = if app.term.is_scrolled_back() {
        format!("{} ↑ scroll {} ", base.trim_end(), app.term.scrollback_offset())
    } else {
        base.to_string()
    };
    let block = panel_block(&title, focused, p);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.selected.is_none() {
        let hint = Paragraph::new("Select a session on the left and press Space.")
            .style(Style::default().fg(p.muted).bg(p.panel_bg))
            .wrap(Wrap { trim: true });
        f.render_widget(hint, inner);
        return;
    }

    // Stopped/crashed session in the focused pane: nothing useful to
    // render from the (closed) WS, and "press u to start" is hard to
    // discover without first switching to tree focus. Surface the
    // start affordance front-and-centre so Enter / u kick the agent
    // back to life from right here.
    if let Some(s) = app.selected_session()
        && matches!(s.status, SessionStatus::Stopped | SessionStatus::Crashed)
    {
        let label = if matches!(s.status, SessionStatus::Crashed) {
            "crashed"
        } else {
            "stopped"
        };
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  ● {} — `{}`", label, s.name),
                Style::default().fg(p.warning).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  press  u  or  Enter  to start",
                Style::default().fg(p.fg_strong),
            )),
            Line::from(Span::styled(
                "  Ctrl-E to focus the tree · Shift-D to remove",
                Style::default().fg(p.muted),
            )),
        ];
        let hint = Paragraph::new(lines)
            .style(Style::default().bg(p.panel_bg))
            .wrap(Wrap { trim: false });
        f.render_widget(hint, inner);
        return;
    }

    let pseudo = PseudoTerminal::new(app.term.screen());
    f.render_widget(pseudo, inner);
    fill_default_bg(f, inner, p.panel_bg);
    overlay_term_selection(f, inner, app, Side::Left);
}

fn draw_terminal_right(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let focused = app.focus == Focus::TermRight;
    let base = if focused {
        " right · Ctrl-E ↔ tree · Ctrl-W close · type freely "
    } else {
        " right · Tab focus · Ctrl-W close "
    };
    let Some(slot) = app.split_right.as_ref() else {
        return; // shouldn't happen — only drawn while split is open
    };
    let title = if slot.term.is_scrolled_back() {
        format!("{} ↑ scroll {} ", base.trim_end(), slot.term.scrollback_offset())
    } else {
        base.to_string()
    };
    let block = panel_block(&title, focused, p);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if slot.selected.is_none() {
        let hint = Paragraph::new("Pick a session for this pane (focus, then Ctrl-P).")
            .style(Style::default().fg(p.muted).bg(p.panel_bg))
            .wrap(Wrap { trim: true });
        f.render_widget(hint, inner);
        return;
    }
    let pseudo = PseudoTerminal::new(slot.term.screen());
    f.render_widget(pseudo, inner);
    fill_default_bg(f, inner, p.panel_bg);
    overlay_term_selection(f, inner, app, Side::Right);
}

fn draw_lazygit(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let focused = app.focus == Focus::Lazygit;
    let title = if focused {
        " 3 lazygit · Ctrl-E ↔ tree · Ctrl-G close "
    } else {
        " 3 lazygit · 3 / Tab focus · Ctrl-G close "
    };
    let block = panel_block(title, focused, p);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(lg) = app.lazygit.as_ref() {
        let pseudo = PseudoTerminal::new(lg.screen());
        f.render_widget(pseudo, inner);
        fill_default_bg(f, inner, p.panel_bg);
    } else {
        let hint = Paragraph::new("lazygit not running")
            .style(Style::default().fg(p.muted).bg(p.panel_bg));
        f.render_widget(hint, inner);
    }
}

/// Fill any cells in `area` whose background is `Color::Reset` with `bg`.
///
/// `tui-term`'s `PseudoTerminal` always `Clear`s its area first and then
/// writes vt100 cells via `set_bg(...)` — empty vt100 cells map to
/// `Color::Reset`, which on a transparent terminal lets the host wallpaper
/// bleed through. This patch reasserts the theme's `panel_bg` over those
/// otherwise-default cells while leaving any explicitly-coloured cell
/// (claude's status line, syntax highlighting, etc.) untouched. No-ops
/// for the `system` theme where `panel_bg == Color::Reset` by design.
fn fill_default_bg(f: &mut Frame<'_>, area: Rect, bg: Color) {
    if bg == Color::Reset {
        return;
    }
    let buf = f.buffer_mut();
    let x_end = area.x.saturating_add(area.width);
    let y_end = area.y.saturating_add(area.height);
    for y in area.y..y_end {
        for x in area.x..x_end {
            if let Some(cell) = buf.cell_mut((x, y))
                && cell.bg == Color::Reset
            {
                cell.set_bg(bg);
            }
        }
    }
}

/// Paint an inverted-colour highlight over the cells covered by an
/// active mouse selection. Runs after the `PseudoTerminal` widget so
/// it stamps directly on the rendered cells via `Modifier::REVERSED`,
/// which preserves the underlying glyph + colour and just flips fg/bg
/// — exactly the look users expect from xterm/iTerm/Alacritty's
/// native selection.
///
/// `inner` is the pane's content rect (post-border). Selection coords
/// are 1-based pane-local; we offset by (`inner.x`, `inner.y`) and
/// clamp into the rect. No-op when the selection belongs to a
/// different pane or doesn't exist.
fn overlay_term_selection(f: &mut Frame<'_>, inner: Rect, app: &App, side: Side) {
    let Some(sel) = app.term_selection else {
        return;
    };
    if sel.side != side {
        return;
    }
    let ((s_col, s_row), (e_col, e_row)) = sel.ordered();
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let max_col = inner.width;
    let max_row = inner.height;
    // 1-based → 0-based, clamp into the visible content rect.
    let s_row0 = s_row.saturating_sub(1).min(max_row.saturating_sub(1));
    let e_row0 = e_row.saturating_sub(1).min(max_row.saturating_sub(1));
    let s_col0 = s_col.saturating_sub(1).min(max_col.saturating_sub(1));
    let e_col0 = e_col.saturating_sub(1).min(max_col.saturating_sub(1));

    let buf = f.buffer_mut();
    for r in s_row0..=e_row0 {
        let (col_lo, col_hi) = if s_row0 == e_row0 {
            (s_col0.min(e_col0), s_col0.max(e_col0))
        } else if r == s_row0 {
            (s_col0, max_col.saturating_sub(1))
        } else if r == e_row0 {
            (0, e_col0)
        } else {
            (0, max_col.saturating_sub(1))
        };
        for c in col_lo..=col_hi {
            let x = inner.x + c;
            let y = inner.y + r;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.modifier.insert(Modifier::REVERSED);
            }
        }
    }
}

fn draw_status(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    // Each chip is opt-in/out via `app.prefs`. Build two lists — the
    // workdir/tool context chips anchor left along with the live
    // connection + throughput indicators (so connection state sits next
    // to the path it applies to, not on the opposite side of the bar).
    // Everything else (totals, hints, transient status messages) is
    // right-aligned so noisier chips sit far from the eye when scanning
    // the workdir.
    let mut left: Vec<Span<'static>> = Vec::with_capacity(6);
    let mut right: Vec<Span<'static>> = Vec::with_capacity(10);

    if app.prefs.get(StatusChip::Workdir) {
        let workdir = app
            .selected_session()
            .map(|s| collapse_home(&s.workdir))
            .unwrap_or_else(|| "—".to_string());
        left.push(Span::styled(
            format!(" {workdir} "),
            Style::default().fg(p.fg).bg(p.chrome_bg),
        ));
    }

    if app.prefs.get(StatusChip::Tool) {
        let tool_label = app
            .selected_session()
            .map(|s| match s.model.as_deref() {
                Some(m) => format!("{} · {}", s.tool, m),
                None => s.tool.clone(),
            })
            .unwrap_or_else(|| "—".to_string());
        left.push(Span::styled(
            format!("{tool_label} "),
            Style::default().fg(p.muted).bg(p.chrome_bg),
        ));
    }

    if app.prefs.get(StatusChip::Conn) {
        let (conn_label, conn_color) = match app.conn {
            ConnState::Connected => ("● connected", p.success),
            ConnState::Connecting => ("◌ connecting", p.warning),
            ConnState::Reconnecting { .. } => ("⟳ reconnecting", p.warning),
            ConnState::Disconnected => ("✗ disconnected", p.error),
        };
        left.push(Span::styled(
            format!(" {conn_label} "),
            Style::default().fg(conn_color).bg(p.chrome_bg),
        ));
    }

    if app.prefs.get(StatusChip::Io) {
        // Live throughput chip — `↓ rate · ↑ rate`. Treated as one chip
        // visually so it doesn't compete with workdir/tool for width.
        // Renders even when both rates are zero so the user can see the
        // chip exists and learn it's there to flip off.
        let down = fmt_rate(app.io.rate_in());
        let up = fmt_rate(app.io.rate_out());
        left.push(Span::styled(
            format!(" ↓{down} ↑{up} "),
            Style::default().fg(p.accent_alt).bg(p.chrome_bg),
        ));
    }

    if app.prefs.get(StatusChip::IoTotals) {
        // Lifetime totals — quieter, opt-in chip for users who care
        // about absolute volume (e.g. metered networks).
        let din = fmt_bytes(app.io.total_in());
        let dout = fmt_bytes(app.io.total_out());
        right.push(Span::styled(
            format!(" Σ↓{din} ↑{dout} "),
            Style::default().fg(p.muted).bg(p.chrome_bg),
        ));
    }

    if app.prefs.get(StatusChip::Lazygit) && app.lazygit_open() {
        right.push(Span::styled(
            " lazygit ",
            Style::default()
                .bg(p.chip_bg)
                .fg(p.chip_fg)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if app.prefs.get(StatusChip::Theme) {
        right.push(Span::styled(
            format!(" {} ", app.theme.name),
            Style::default().fg(p.chip_fg).bg(p.chip_bg),
        ));
    }

    // Status message is always shown — it's a transient feedback channel
    // and hiding it would silently drop user-relevant signals (e.g.
    // "sidebar hidden", "theme: midnight"). Empty when no message.
    let extra = match &app.status_msg {
        Some(m) => format!(" · {m} "),
        None => String::new(),
    };
    if !extra.is_empty() {
        right.push(Span::styled(
            extra,
            Style::default().fg(p.muted).bg(p.chrome_bg),
        ));
    }

    if app.prefs.get(StatusChip::PaletteHint) {
        right.push(Span::styled(
            " Ctrl-P palette ",
            Style::default().fg(p.accent).bg(p.chrome_bg),
        ));
    }
    if app.prefs.get(StatusChip::HelpHint) {
        right.push(Span::styled(
            " ? help ",
            Style::default().fg(p.muted).bg(p.chrome_bg),
        ));
    }

    // Left paragraph paints the whole bar's chrome_bg; the right
    // paragraph then overdraws its slot. Cells the right doesn't touch
    // keep the chrome_bg fill, so the gap between left and right
    // matches the bar instead of bleeding through to the host bg.
    let bg = Style::default().bg(p.chrome_bg).fg(p.fg);
    let left_p = Paragraph::new(Line::from(left))
        .style(bg)
        .alignment(Alignment::Left);
    let right_p = Paragraph::new(Line::from(right)).alignment(Alignment::Right);
    f.render_widget(left_p, area);
    f.render_widget(right_p, area);
}

/// Bottom-left toast stack. Renders as an overlay (not a layout slot)
/// so toasts coming and going never reflow `Areas` — the terminal pane
/// stays glued to its size and we avoid the jitter that would come
/// from constantly resizing the inner vt100 grid.
///
/// Layout: each toast is a bordered block with a severity-coloured
/// border, anchored against the left edge with a 1-cell margin. They
/// stack upward from just above the status bar (newest on top).
fn draw_notifications(f: &mut Frame<'_>, screen: Rect, app: &App, p: &Palette) {
    if screen.width < 6 || screen.height < 4 {
        return;
    }

    // Status bar consumes the bottom row; the stack starts one row above.
    let max_width = screen.width.saturating_sub(2).min(48);
    let mut bottom = screen.bottom().saturating_sub(1);
    // Render newest on top: walk from newest (last) downward in the
    // stack so the most recent event sits highest in the column.
    for n in app.notifications.iter().rev() {
        let height = toast_height(n, max_width);
        if height + 1 > bottom.saturating_sub(screen.y) {
            // No more vertical room above the status bar — drop the rest.
            break;
        }
        let y = bottom.saturating_sub(height);
        let rect = Rect {
            x: screen.x.saturating_add(1),
            y,
            width: max_width,
            height,
        };
        f.render_widget(Clear, rect);
        f.render_widget(toast_widget(n, p), rect);
        bottom = y;
    }
}

/// Compute how many rows a toast needs given its width budget. Always
/// reserves 2 rows for the top + bottom border plus the title row, then
/// adds wrap-budgeted body lines if `body` is set.
fn toast_height(n: &Notification, width: u16) -> u16 {
    // Inside the borders we have `width - 2` columns for text.
    let inner = width.saturating_sub(2).max(1) as usize;
    let title_rows = wrap_rows(&n.title, inner);
    let body_rows = match &n.body {
        Some(b) if !b.is_empty() => wrap_rows(b, inner),
        _ => 0,
    };
    let content = (title_rows + body_rows).max(1) as u16;
    // 2 border rows + content. Cap at 6 so a runaway body doesn't eat
    // the whole screen.
    (2 + content).min(6)
}

fn wrap_rows(s: &str, width: usize) -> usize {
    if width == 0 || s.is_empty() {
        return 1;
    }
    s.chars().count().div_ceil(width).max(1)
}

fn toast_widget<'a>(n: &'a Notification, p: &Palette) -> Paragraph<'a> {
    let color = match n.kind {
        NotifKind::Info => p.accent,
        NotifKind::Warn => p.warning,
        NotifKind::Error => p.error,
    };
    let icon = match n.kind {
        NotifKind::Info => "●",
        NotifKind::Warn => "▲",
        NotifKind::Error => "✗",
    };
    let mut lines: Vec<Line> = Vec::with_capacity(2);
    lines.push(Line::from(vec![
        Span::styled(format!("{icon} "), Style::default().fg(color)),
        Span::styled(
            n.title.as_str(),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ),
    ]));
    if let Some(body) = n.body.as_deref()
        && !body.is_empty()
    {
        lines.push(Line::from(Span::styled(
            body,
            Style::default().fg(p.muted),
        )));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(p.surface_bg));
    Paragraph::new(lines).block(block).wrap(Wrap { trim: false })
}

fn draw_help_overlay(f: &mut Frame<'_>, area: Rect, lazygit_open: bool, p: &Palette) {
    let mut lines = vec![
        head("agentum terminal — keys", p),
        Line::from(""),
        head("  Universal (work even inside the terminal pane)", p),
        body("  Ctrl-P / Ctrl-Shift-P  command palette", p),
        body("  Ctrl-E            toggle focus: tree ↔ terminal", p),
        body("  Ctrl-G            toggle lazygit side pane", p),
        body("  Ctrl-Tab          flip back to last session", p),
        body("  Ctrl-B            toggle the sidebar tree", p),
        body("  Ctrl-T            toggle the agent plan/todo/task panel", p),
        body("  Ctrl-K Z          toggle fullscreen (zen)", p),
        body("  Ctrl-K , / .      shrink / grow lazygit width", p),
        body("  Ctrl-\\            split the focused terminal pane", p),
        body("  Ctrl-W            close the split", p),
        body("  Ctrl-Shift-←/→    resize the split divider (when split is open)", p),
        body("  Ctrl-,            settings (notifications · layout · status bar)", p),
        body("  Ctrl-R            rename the highlighted session (tree only)", p),
        body("  Mouse wheel       scroll the pane under the cursor", p),
        body("  Shift-PgUp/PgDn   scroll the focused pane (no mouse needed)", p),
        body("  F5                next panel", p),
        body("  F6                previous panel", p),
        body("  Ctrl-1 … Ctrl-9   jump to Nth project group in the tree", p),
        body("  Ctrl-Q            quit", p),
        body("  Ctrl-C            interrupt focused pane (else quit)", p),
        Line::from(""),
        head("  Tree", p),
        body("  1 / 2 / 3         focus tree / terminal / lazygit", p),
        body("  Tab               next panel", p),
        body("  Shift-Tab         previous panel", p),
        body("  Ctrl-F            filter sessions by name (Esc clears)", p),
        body("  j / k / ↑ / ↓     move selection", p),
        body("  h / l / ← / →     collapse / expand group", p),
        body(
            "  Space             select session and focus the terminal",
            p,
        ),
        body("  Enter             multi-select (WIP — coming soon)", p),
        body("  r                 refresh sessions", p),
        body("  t                 spawn plain bash terminal", p),
        body("  !                 view recent error log", p),
        Line::from(""),
        head("  Terminal", p),
        body(
            "  All other keys forward to the running process (claude code, shell, …)",
            p,
        ),
        Line::from(""),
        head("  Sessions", p),
        body(
            "  n                 new session (name / workdir / tool / model)",
            p,
        ),
        body("  u                 start (up) the selected session", p),
        body("  s                 stop the selected session (graceful)", p),
        body(
            "  K · x · D         kill the selected session (closes & removes)",
            p,
        ),
        Line::from(""),
        head("  Extensions & appearance", p),
        body("  g                 toggle lazygit side pane", p),
        body("  G                 lazygit cheat sheet", p),
        body("  T                 cycle theme", p),
        body(
            "  Ctrl-P then ~     status bar settings (toggle each chip individually)",
            p,
        ),
        body("  ↓ rate ↑ rate     live WS throughput · toggle via ~ I/O speeds", p),
        body(
            "  Shift-F           toggle fullscreen (hide tree + chrome)",
            p,
        ),
        body("  Esc               exit fullscreen", p),
        body("  + / -             widen / narrow sidebar tree", p),
        Line::from(""),
        body("  ?                 toggle this help", p),
        body("  Ctrl-Q            quit (works from any focus)", p),
    ];
    if lazygit_open {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  (lazygit is open — keys forward to it when focused)",
            Style::default().fg(p.muted),
        )));
    }
    overlay_box(f, area, " help ", lines, 70, p);
}

fn draw_cheatsheet_overlay(f: &mut Frame<'_>, area: Rect, ext: &Extension, p: &Palette) {
    let mut lines = vec![
        head(&format!("{} — cheat sheet", ext.display_name), p),
        Line::from(""),
    ];
    for (label, keys) in ext.cheatsheet {
        lines.push(body(&format!("  {label:<22} {keys}"), p));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", ext.homepage),
        Style::default().fg(p.muted),
    )));
    lines.push(Line::from(""));
    lines.push(body("  Esc / Enter to dismiss", p));
    overlay_box(f, area, &format!(" {} ", ext.display_name), lines, 64, p);
}

fn draw_install_overlay(f: &mut Frame<'_>, area: Rect, ext: &Extension, p: &Palette) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} not found on PATH", ext.display_name),
            Style::default().fg(p.warning).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(ext.blurb, Style::default().fg(p.fg))),
        Line::from(""),
        head("Install with one of:", p),
    ];
    for (name, cmd) in extensions::install_hints(ext) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {name:<22} "), Style::default().fg(p.accent)),
            Span::styled(cmd, Style::default().fg(p.fg)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", ext.homepage),
        Style::default().fg(p.muted),
    )));
    lines.push(Line::from(""));
    lines.push(body("  Esc / Enter to dismiss", p));
    overlay_box(
        f,
        area,
        &format!(" install {} ", ext.display_name),
        lines,
        72,
        p,
    );
}

fn draw_palette_overlay(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let cat = palette_catalog(app);
    let (mode, filtered) = cat.filtered(&app.palette.query);
    let max = filtered.len().saturating_sub(1);
    let cursor = app.palette.cursor.min(max);

    let w = 80.min(area.width.saturating_sub(4));
    let h = 24.min(area.height.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let r = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let block = Block::default()
        .title(Span::styled(
            format!(" command palette · {} ", mode.label()),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus_border))
        .style(Style::default().bg(p.surface_bg).fg(p.fg));
    f.render_widget(Clear, r);
    f.render_widget(block.clone(), r);

    let inner = block.inner(r);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // query
            Constraint::Min(1),    // results
            Constraint::Length(1), // hints
        ])
        .split(inner);

    // Query line — mode chip + typed bytes + cursor.
    let mode_chip = Span::styled(
        format!(" {} ", mode.label()),
        Style::default()
            .fg(p.chip_fg)
            .bg(p.chip_bg)
            .add_modifier(Modifier::BOLD),
    );
    let query_line = Line::from(vec![
        Span::styled(" › ", Style::default().fg(p.accent)),
        mode_chip,
        Span::raw(" "),
        Span::styled(
            app.palette.query.clone(),
            Style::default()
                .fg(p.fg_strong)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("█", Style::default().fg(p.accent)),
    ]);
    let query = Paragraph::new(query_line).style(Style::default().bg(p.surface_bg));
    f.render_widget(query, rows[0]);

    // Action list.
    let visible = (rows[1].height as usize).max(1);
    let start = cursor.saturating_sub(visible.saturating_sub(1));
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, a)| {
            let is_cursor = i == cursor;
            let row_style = if is_cursor {
                Style::default()
                    .bg(p.cursor_bg)
                    .fg(p.cursor_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(p.surface_bg).fg(p.fg)
            };
            let pointer = if is_cursor { "›" } else { "│" };
            let pointer_style = if is_cursor {
                Style::default().fg(p.accent)
            } else {
                Style::default().fg(p.subtle)
            };
            let group = format!(" {:<10}", a.group);
            let label = format!(" {} ", a.label);
            let hint = if a.hint.is_empty() {
                String::new()
            } else {
                format!("  {}", a.hint)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{pointer} "), pointer_style),
                Span::styled(group, Style::default().fg(p.muted)),
                Span::raw(label),
                Span::styled(hint, Style::default().fg(p.muted)),
            ]))
            .style(row_style)
        })
        .collect();

    if items.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no matches — Esc to close",
            Style::default().fg(p.muted),
        )))
        .style(Style::default().bg(p.surface_bg));
        f.render_widget(empty, rows[1]);
    } else {
        let list = List::new(items).style(Style::default().bg(p.surface_bg));
        f.render_widget(list, rows[1]);
    }

    // Hints line — Fresh-style prefix legend. `~` is the new
    // settings prefix; truncated on narrow palettes by ratatui's
    // line clip rather than by us pre-trimming.
    let hints = Line::from(vec![
        Span::styled(" › ", Style::default().fg(p.subtle)),
        Span::styled("type", Style::default().fg(p.muted)),
        Span::styled("  >", Style::default().fg(p.accent)),
        Span::styled(" commands", Style::default().fg(p.muted)),
        Span::styled("  #", Style::default().fg(p.accent)),
        Span::styled(" sessions", Style::default().fg(p.muted)),
        Span::styled("  @", Style::default().fg(p.accent)),
        Span::styled(" themes", Style::default().fg(p.muted)),
        Span::styled("  ~", Style::default().fg(p.accent)),
        Span::styled(" settings", Style::default().fg(p.muted)),
        Span::styled("    ↑↓ ", Style::default().fg(p.subtle)),
        Span::styled("move", Style::default().fg(p.muted)),
        Span::styled("  ⏎ ", Style::default().fg(p.subtle)),
        Span::styled("run", Style::default().fg(p.muted)),
        Span::styled("  Esc ", Style::default().fg(p.subtle)),
        Span::styled("close", Style::default().fg(p.muted)),
    ]);
    let hints_para = Paragraph::new(hints).style(Style::default().bg(p.chrome_bg));
    f.render_widget(hints_para, rows[2]);
}

/// One-line label for a numeric ms value rendered in the overlay.
fn fmt_ttl_ms(ms: u64) -> String {
    if ms % 1000 == 0 {
        format!("{}s", ms / 1000)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

fn settings_row_label(row: SettingsRow, prefs: &Prefs) -> (String, String) {
    let onoff = |b: bool| if b { "on" } else { "off" };
    match row {
        SettingsRow::SoundMaster => (
            "  Sound: master".into(),
            onoff(prefs.sound_master).into(),
        ),
        SettingsRow::SoundInfo => ("  Sound: info".into(), onoff(prefs.sound_info).into()),
        SettingsRow::SoundWarn => ("  Sound: warn".into(), onoff(prefs.sound_warn).into()),
        SettingsRow::SoundError => ("  Sound: error".into(), onoff(prefs.sound_error).into()),
        SettingsRow::TtlInfo => (
            "  Notification TTL: info".into(),
            fmt_ttl_ms(prefs.ttl_ms(SoundKind::Info)),
        ),
        SettingsRow::TtlWarn => (
            "  Notification TTL: warn".into(),
            fmt_ttl_ms(prefs.ttl_ms(SoundKind::Warn)),
        ),
        SettingsRow::TtlError => (
            "  Notification TTL: error".into(),
            fmt_ttl_ms(prefs.ttl_ms(SoundKind::Error)),
        ),
        SettingsRow::SidebarHidden => (
            "  Sidebar (tree)".into(),
            onoff(!prefs.sidebar_hidden).into(),
        ),
        SettingsRow::RightPanelVisible => (
            "  Agent panel (right)".into(),
            onoff(prefs.right_panel_visible).into(),
        ),
        SettingsRow::ChipWorkdir => ("  Chip: workdir".into(), onoff(prefs.show_workdir).into()),
        SettingsRow::ChipTool => ("  Chip: tool".into(), onoff(prefs.show_tool).into()),
        SettingsRow::ChipConn => ("  Chip: connection".into(), onoff(prefs.show_conn).into()),
        SettingsRow::ChipLazygit => ("  Chip: lazygit".into(), onoff(prefs.show_lazygit).into()),
        SettingsRow::ChipTheme => ("  Chip: theme".into(), onoff(prefs.show_theme).into()),
        SettingsRow::ChipIo => ("  Chip: I/O speeds".into(), onoff(prefs.show_io).into()),
        SettingsRow::ChipIoTotals => (
            "  Chip: I/O totals".into(),
            onoff(prefs.show_io_totals).into(),
        ),
        SettingsRow::ChipPaletteHint => (
            "  Chip: palette hint".into(),
            onoff(prefs.show_palette_hint).into(),
        ),
        SettingsRow::ChipHelpHint => (
            "  Chip: help hint".into(),
            onoff(prefs.show_help_hint).into(),
        ),
        SettingsRow::ResetAll => (
            "  Reset everything to defaults".into(),
            "press space".into(),
        ),
    }
}

fn draw_settings_overlay(
    f: &mut Frame<'_>,
    area: Rect,
    state: &SettingsState,
    prefs: &Prefs,
    p: &Palette,
) {
    // Vertical list of label + value with section headers between
    // groups. Highlighted row painted with `chip_bg` so it reads as
    // "selected" without inventing a new palette tone.
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head("Settings", p));
    lines.push(Line::from(""));

    let cursor = state.cursor.min(SettingsRow::ROWS.len() - 1);
    let label_w = 64usize;
    for (i, row) in SettingsRow::ROWS.iter().copied().enumerate() {
        if let Some(header) = row.section_header() {
            if i != 0 {
                lines.push(Line::from(""));
            }
            lines.push(head(header, p));
        }
        let (label, value) = settings_row_label(row, prefs);
        let pad = label_w
            .saturating_sub(label.chars().count())
            .saturating_sub(value.chars().count());
        let pad = " ".repeat(pad.max(2));
        let label_style = if i == cursor {
            Style::default().fg(p.fg_strong).bg(p.chip_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        let value_style = if i == cursor {
            Style::default().fg(p.accent).bg(p.chip_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.muted)
        };
        let arrow = if i == cursor { " ▸" } else { "  " };
        let row_line = Line::from(vec![
            Span::styled(arrow.to_string(), Style::default().fg(p.accent)),
            Span::styled(label, label_style),
            Span::styled(pad, label_style),
            Span::styled(value, value_style),
        ]);
        lines.push(row_line);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ move · ←→ adjust · space toggle · r reset row · Esc close".to_string(),
        Style::default().fg(p.muted),
    )));

    overlay_box(f, area, " settings ", lines, (label_w as u16) + 8, p);
}

/// Inline rename prompt. Compact: just the buffer (with a fake cursor
/// block) and an optional inline error. Modeled on the existing
/// confirm overlay's footprint so it doesn't dominate the screen.
fn draw_rename_overlay(f: &mut Frame<'_>, area: Rect, state: &RenameState, p: &Palette) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head("Rename session", p));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  current: {}", state.original),
        Style::default().fg(p.muted),
    )));
    let buffer = if state.buffer.is_empty() {
        "  ▎".to_string()
    } else {
        format!("  {}▎", state.buffer)
    };
    lines.push(Line::from(Span::styled(
        buffer,
        Style::default()
            .fg(p.fg_strong)
            .add_modifier(Modifier::BOLD),
    )));
    if let Some(err) = state.error.as_ref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {err}"),
            Style::default().fg(p.error),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter save · Esc cancel".to_string(),
        Style::default().fg(p.muted),
    )));
    overlay_box(f, area, " rename ", lines, 60, p);
}

fn draw_profiles_overlay(
    f: &mut Frame<'_>,
    area: Rect,
    state: &ProfilesOverlay,
    active: Option<&str>,
    p: &Palette,
) {
    // Two visual modes: a list of profiles, or the inline add-form.
    // Shared frame so the size doesn't jump between them.
    if let Some(form) = &state.add_form {
        draw_profiles_add_form(f, area, form, p);
        return;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head("Endpoints", p));
    lines.push(Line::from(""));

    if let Some(err) = state.error.as_ref() {
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {err}"),
            Style::default().fg(p.error),
        )));
        lines.push(Line::from(""));
    }

    if state.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no profiles defined".to_string(),
            Style::default().fg(p.muted),
        )));
        lines.push(Line::from(Span::styled(
            "  press `a` to add the first one".to_string(),
            Style::default().fg(p.muted),
        )));
    } else {
        for (i, entry) in state.entries.iter().enumerate() {
            let selected = i == state.cursor;
            let is_active = active == Some(entry.name.as_str());
            // Marker conveys two facts: which one's selected (▶) and
            // which one's the active connection (●). The default
            // pointer surfaces as a separate "default" suffix.
            let marker: &str = if selected { "▶ " } else { "  " };
            let active_dot: &str = if is_active { "● " } else { "  " };
            let mut row_spans = vec![
                Span::styled(
                    marker.to_string(),
                    Style::default()
                        .fg(if selected { p.accent } else { p.muted })
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(active_dot.to_string(), Style::default().fg(p.success)),
                Span::styled(
                    entry.name.clone(),
                    Style::default()
                        .fg(if selected { p.fg_strong } else { p.fg })
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            if entry.is_default {
                row_spans.push(Span::styled(
                    "  · default".to_string(),
                    Style::default().fg(p.accent),
                ));
            }
            if entry.fingerprint.is_some() {
                row_spans.push(Span::styled(
                    "  · pinned".to_string(),
                    Style::default().fg(p.muted),
                ));
            }
            lines.push(Line::from(row_spans));
            lines.push(Line::from(Span::styled(
                format!("    {}", entry.url),
                Style::default().fg(p.muted),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter switch · a add · d remove · Esc close".to_string(),
        Style::default().fg(p.muted),
    )));

    overlay_box(f, area, " endpoints ", lines, 80, p);
}

fn draw_profiles_add_form(f: &mut Frame<'_>, area: Rect, form: &AddProfileForm, p: &Palette) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head("Add endpoint", p));
    lines.push(Line::from(""));
    push_form_field(
        &mut lines,
        "Name",
        &form.name,
        form.field == AddProfileField::Name,
        "vps",
        p,
    );
    push_form_field(
        &mut lines,
        "URL",
        &form.url,
        form.field == AddProfileField::Url,
        "https://my-vps.example.com:8822",
        p,
    );
    push_form_field_with_hint(
        &mut lines,
        "Fingerprint",
        &form.fingerprint,
        form.field == AddProfileField::Fingerprint,
        "AB:CD:…",
        Some("(optional — leave blank to prompt on first connect)"),
        p,
    );
    push_toggle_field(
        &mut lines,
        "Set as default",
        form.set_default,
        form.field == AddProfileField::SetDefault,
        p,
    );
    if let Some(err) = form.error.as_ref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {err}"),
            Style::default().fg(p.error),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Tab next · Space toggle · Enter save · Esc back".to_string(),
        Style::default().fg(p.muted),
    )));
    overlay_box(f, area, " add endpoint ", lines, 80, p);
}

fn draw_errors_overlay(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let w = 100.min(area.width.saturating_sub(4));
    let h = 28.min(area.height.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let r = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let title = if app.errors.is_empty() {
        " errors ".to_string()
    } else {
        format!(
            " errors · showing {} of {} ",
            app.errors.len(),
            app.error_count
        )
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.error).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus_border))
        .style(Style::default().bg(p.surface_bg).fg(p.fg));
    f.render_widget(Clear, r);
    f.render_widget(block.clone(), r);
    let inner = block.inner(r);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    if app.errors.is_empty() {
        let line = Line::from(vec![Span::styled(
            "  no errors recorded — agentum will list failures here as they happen",
            Style::default().fg(p.muted),
        )]);
        let para = Paragraph::new(line).style(Style::default().bg(p.surface_bg));
        f.render_widget(para, rows[0]);
    } else {
        // Newest-first list. `errors_scroll` is "entries from the top
        // (newest) we've already scrolled past". Saturate against the
        // list length so `End` (errors_scroll = usize::MAX) snaps to
        // the oldest visible entry without crashing.
        let visible = (rows[0].height as usize).max(1);
        let n = app.errors.len();
        let max_scroll = n.saturating_sub(1);
        let scroll = app.errors_scroll.min(max_scroll);
        let text_w = rows[0].width.saturating_sub(12) as usize; // leave room for stamp
        let lines: Vec<Line> = app
            .errors
            .iter()
            .rev()
            .skip(scroll)
            .take(visible)
            .map(|e| format_error_line(e, text_w, p))
            .collect();
        let para = Paragraph::new(lines).style(Style::default().bg(p.surface_bg));
        f.render_widget(para, rows[0]);
    }

    let hints = Line::from(vec![
        Span::styled(" j/k ", Style::default().fg(p.subtle)),
        Span::styled("scroll", Style::default().fg(p.muted)),
        Span::styled("  PgUp/PgDn ", Style::default().fg(p.subtle)),
        Span::styled("page", Style::default().fg(p.muted)),
        Span::styled("  g/G ", Style::default().fg(p.subtle)),
        Span::styled("top/bottom", Style::default().fg(p.muted)),
        Span::styled("  c ", Style::default().fg(p.subtle)),
        Span::styled("clear", Style::default().fg(p.muted)),
        Span::styled("  Esc / e ", Style::default().fg(p.subtle)),
        Span::styled("close", Style::default().fg(p.muted)),
    ]);
    let hints_para = Paragraph::new(hints).style(Style::default().bg(p.surface_bg));
    f.render_widget(hints_para, rows[1]);
}

/// Render one error entry as a single line: `[ 12s ] message`.
/// Long messages are truncated to the available width so wrapping
/// doesn't desync the scroll offset (which counts entries, not lines).
fn format_error_line<'a>(entry: &ErrorEntry, text_w: usize, p: &Palette) -> Line<'a> {
    let stamp = format_short_age(entry.at);
    let stamp_text = format!("  [{stamp:>4}] ");
    let mut text = entry.text.replace('\n', " ");
    if text_w > 1 && text.chars().count() > text_w {
        text = text.chars().take(text_w.saturating_sub(1)).collect::<String>() + "…";
    }
    Line::from(vec![
        Span::styled(stamp_text, Style::default().fg(p.muted)),
        Span::styled(text, Style::default().fg(p.fg)),
    ])
}

/// Compact "time since" label: `12s`, `3m`, `2h`, `4d`. Avoids needing
/// the local-offset feature of the `time` crate (the workspace doesn't
/// enable it) and stays readable on a chrome row.
fn format_short_age(at: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(at)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn draw_new_session_overlay(
    f: &mut Frame<'_>,
    area: Rect,
    form: &NewSessionForm,
    tool_unavailable: bool,
    p: &Palette,
) {
    // If the directory picker is up, it owns the overlay box.
    if let Some(picker) = &form.picker {
        draw_dir_picker_overlay(f, area, picker, p);
        return;
    }

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(head("New session", p));
    lines.push(Line::from(""));

    push_form_field(
        &mut lines,
        "Name",
        &form.name,
        form.field == NewSessionField::Name,
        "alpha",
        p,
    );
    // The Tool field's hint normally lists the cycle order; when the
    // typed name resolves to an uninstalled binary the hint is
    // replaced by a red warning so the user sees the gating reason
    // without having to submit and wait for an error toast. Mirrors
    // the tile-dimming on the dashboard.
    if tool_unavailable {
        push_form_field_with_warn_hint(
            &mut lines,
            "Tool",
            &form.tool,
            form.field == NewSessionField::Tool,
            "claude",
            "(not installed on the daemon)",
            p,
        );
    } else {
        push_form_field_with_hint(
            &mut lines,
            "Tool",
            &form.tool,
            form.field == NewSessionField::Tool,
            "claude",
            Some("Tab cycles claude → codex → cursor → opencode → aider"),
            p,
        );
    }
    push_form_field_with_hint(
        &mut lines,
        "Model",
        &form.model,
        form.field == NewSessionField::Model,
        "e.g. claude-opus-4-7",
        Some("(optional)"),
        p,
    );
    push_form_field_with_hint(
        &mut lines,
        "Working directory",
        &form.workdir,
        form.field == NewSessionField::Workdir,
        "~/projects/foo",
        Some("Tab autocompletes · Enter opens the folder picker"),
        p,
    );
    push_form_field_with_hint(
        &mut lines,
        "Extra args",
        &form.args,
        form.field == NewSessionField::Args,
        "key=value pairs, space-separated",
        Some("e.g. resume=true model=sonnet"),
        p,
    );
    push_toggle_field(
        &mut lines,
        "Start immediately (--up)",
        form.up_after,
        form.field == NewSessionField::UpAfter,
        p,
    );
    let yolo_supported = form.yolo_active() || form.yolo;
    let yolo_label = if !yolo_supported && form.yolo {
        // User toggled it on but the current tool doesn't have a known
        // YOLO flag — adapter will drop the marker at launch.
        "YOLO mode (skip permission prompts — ignored for this tool)"
    } else {
        // Generic — actual flag is tool-specific (claude:
        // --dangerously-skip-permissions, codex:
        // --dangerously-bypass-approvals-and-sandbox, gemini: --yolo).
        "YOLO mode (skip permission prompts)"
    };
    push_toggle_field(
        &mut lines,
        yolo_label,
        form.yolo,
        form.field == NewSessionField::Yolo,
        p,
    );

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  Tab/↓", Style::default().fg(p.accent)),
        Span::styled(" next   ", Style::default().fg(p.muted)),
        Span::styled("Shift-Tab/↑", Style::default().fg(p.accent)),
        Span::styled(" prev   ", Style::default().fg(p.muted)),
        Span::styled("Enter", Style::default().fg(p.accent)),
        Span::styled(" create   ", Style::default().fg(p.muted)),
        Span::styled("Esc", Style::default().fg(p.accent)),
        Span::styled(" cancel", Style::default().fg(p.muted)),
    ]));
    if let Some(err) = &form.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ! {err}"),
            Style::default().fg(p.error),
        )));
    }
    overlay_box(f, area, " New session ", lines, 70, p);
}

fn push_form_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    focused: bool,
    placeholder: &str,
    p: &Palette,
) {
    push_form_field_with_hint(lines, label, value, focused, placeholder, None, p);
}

fn push_form_field_with_hint(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    focused: bool,
    placeholder: &str,
    hint: Option<&str>,
    p: &Palette,
) {
    let label_color = if focused { p.accent } else { p.muted };
    let mut label_spans = vec![Span::styled(
        format!("  {label}"),
        Style::default().fg(label_color).add_modifier(Modifier::BOLD),
    )];
    if let Some(h) = hint {
        label_spans.push(Span::styled(
            format!("  {h}"),
            Style::default().fg(p.muted),
        ));
    }
    lines.push(Line::from(label_spans));
    let value_line = if value.is_empty() && !focused {
        Line::from(Span::styled(
            format!("    {placeholder}"),
            Style::default().fg(p.muted),
        ))
    } else {
        let mut spans = vec![Span::styled(
            format!("    {value}"),
            Style::default().fg(p.fg),
        )];
        if focused {
            spans.push(Span::styled("▍", Style::default().fg(p.accent)));
        }
        Line::from(spans)
    };
    lines.push(value_line);
}

/// Same shape as `push_form_field_with_hint` but renders the hint in
/// the palette's danger color. Used for the Tool field's "(not
/// installed)" callout — see [`draw_new_session_overlay`].
fn push_form_field_with_warn_hint(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    focused: bool,
    placeholder: &str,
    hint: &str,
    p: &Palette,
) {
    let label_color = if focused { p.accent } else { p.muted };
    let label_spans = vec![
        Span::styled(
            format!("  {label}"),
            Style::default().fg(label_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {hint}"),
            Style::default().fg(p.error).add_modifier(Modifier::BOLD),
        ),
    ];
    lines.push(Line::from(label_spans));
    let value_line = if value.is_empty() && !focused {
        Line::from(Span::styled(
            format!("    {placeholder}"),
            Style::default().fg(p.muted),
        ))
    } else {
        let mut spans = vec![Span::styled(
            format!("    {value}"),
            // Tint the value itself to reinforce the warn state. The
            // input is still editable; the colour is purely advisory.
            Style::default().fg(p.error),
        )];
        if focused {
            spans.push(Span::styled("▍", Style::default().fg(p.accent)));
        }
        Line::from(spans)
    };
    lines.push(value_line);
}

fn push_toggle_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    on: bool,
    focused: bool,
    p: &Palette,
) {
    let label_color = if focused { p.accent } else { p.muted };
    let mark: &'static str = if on { "[x]" } else { "[ ]" };
    let mark_color = if on { p.success } else { p.muted };
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(mark, Style::default().fg(mark_color)),
        Span::raw(" "),
        Span::styled(label.to_string(), Style::default().fg(label_color)),
    ];
    if focused {
        spans.push(Span::styled(
            "  · space/Enter to toggle".to_string(),
            Style::default().fg(p.muted),
        ));
    }
    lines.push(Line::from(spans));
}

fn draw_dir_picker_overlay(f: &mut Frame<'_>, area: Rect, picker: &DirPickerState, p: &Palette) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(head("Pick a working directory", p));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  current  ", Style::default().fg(p.muted)),
        Span::styled(picker.path.clone(), Style::default().fg(p.fg).add_modifier(Modifier::BOLD)),
    ]));
    if picker.parent.is_some() {
        lines.push(Line::from(Span::styled(
            "  ←/Backspace  go up",
            Style::default().fg(p.muted),
        )));
    }
    lines.push(Line::from(""));

    if let Some(err) = &picker.error {
        lines.push(Line::from(Span::styled(
            format!("  ! {err}"),
            Style::default().fg(p.error),
        )));
        lines.push(Line::from(""));
    }

    if picker.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no subdirectories)",
            Style::default().fg(p.muted),
        )));
    } else {
        // Show up to 14 entries with the cursor as a > marker.
        let max = picker.entries.len().min(14);
        for (i, entry) in picker.entries.iter().take(max).enumerate() {
            let is_cursor = i == picker.cursor;
            let prefix = if is_cursor { "  > " } else { "    " };
            let style = if is_cursor {
                Style::default().fg(p.cursor_fg).bg(p.cursor_bg)
            } else {
                Style::default().fg(p.fg)
            };
            lines.push(Line::from(Span::styled(
                format!("{prefix}{}/", entry.name),
                style,
            )));
        }
        if picker.entries.len() > max {
            lines.push(Line::from(Span::styled(
                format!("  … {} more", picker.entries.len() - max),
                Style::default().fg(p.muted),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ↑/↓", Style::default().fg(p.accent)),
        Span::styled(" move   ", Style::default().fg(p.muted)),
        Span::styled("→/Enter", Style::default().fg(p.accent)),
        Span::styled(" descend   ", Style::default().fg(p.muted)),
        Span::styled("←", Style::default().fg(p.accent)),
        Span::styled(" up   ", Style::default().fg(p.muted)),
        Span::styled("a", Style::default().fg(p.accent)),
        Span::styled(" use this dir   ", Style::default().fg(p.muted)),
        Span::styled("Esc", Style::default().fg(p.accent)),
        Span::styled(" back", Style::default().fg(p.muted)),
    ]));
    overlay_box(f, area, " Folder picker ", lines, 70, p);
}

fn draw_confirm_overlay(f: &mut Frame<'_>, area: Rect, action: &PendingAction, p: &Palette) {
    let title = if action.is_destructive() {
        " confirm — destructive "
    } else {
        " confirm "
    };
    let prompt_color = if action.is_destructive() {
        p.error
    } else {
        p.fg
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", action.prompt()),
            Style::default().fg(prompt_color),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  y / Enter",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  yes      ", Style::default().fg(p.muted)),
            Span::styled("n / Esc", Style::default().fg(p.accent)),
            Span::styled("  cancel", Style::default().fg(p.muted)),
        ]),
    ];
    overlay_box(f, area, title, lines, 70, p);
}

fn overlay_box(
    f: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: Vec<Line<'_>>,
    width: u16,
    p: &Palette,
) {
    let h = (lines.len() as u16).saturating_add(2);
    let w = width.min(area.width);
    let h = h.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let r = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    let block = Block::default()
        .title(Span::styled(
            title.to_string(),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus_border))
        .style(Style::default().bg(p.surface_bg).fg(p.fg));
    let para = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(p.surface_bg).fg(p.fg));
    f.render_widget(Clear, r);
    f.render_widget(para, r);
}

fn draw_reconnect_overlay(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let (title, body_line) = match app.conn {
        ConnState::Reconnecting { attempt, delay_ms } => {
            let secs = delay_ms as f64 / 1000.0;
            (
                " reconnecting ",
                format!("  attempt {attempt} · retrying in {secs:.1}s  "),
            )
        }
        ConnState::Disconnected => (
            " disconnected ",
            "  connection lost — reconnecting...  ".to_string(),
        ),
        _ => return,
    };
    let dots = match app.tick_count % 4 {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    };
    // Title already says "reconnecting"; don't repeat it as a heading.
    // Keep just the explanation line, the attempt/countdown, and the
    // animated dots tacked onto the countdown so the user still gets
    // visible "this is alive" feedback.
    let lines = vec![
        Line::from(Span::styled(
            "  connection to the agentum daemon was lost",
            Style::default().fg(p.fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{body_line}{dots}"),
            Style::default().fg(p.warning),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Ctrl-Q to quit",
            Style::default().fg(p.muted),
        )),
    ];
    overlay_box(f, area, title, lines, 54, p);
}

fn head(text: &str, p: &Palette) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(p.accent_alt)
            .add_modifier(Modifier::BOLD),
    ))
}

fn body(text: &str, p: &Palette) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), Style::default().fg(p.fg)))
}

fn border_style(focused: bool, p: &Palette) -> Style {
    if focused {
        Style::default().fg(p.focus_border).bg(p.panel_bg)
    } else {
        Style::default().fg(p.idle_border).bg(p.panel_bg)
    }
}

fn collapse_home(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && path.starts_with(&home)
    {
        return format!("~{}", &path[home.len()..]);
    }
    path.to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

// ---------- agent-tasks right panel ----------

/// Right-edge column showing the selected agent's plan, todos, and
/// background tasks as three independent boxes stacked vertically. Each
/// box owns its own border and title so they read as distinct cards
/// rather than sub-sections of a wrapper. Status is rendered with
/// `[x] / [~] / [ ]` prefix badges so it reads cleanly even on
/// terminals with limited colour.
fn draw_agent_tasks_panel(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Three equal-height boxes. Saturating-divide so we still produce
    // sensible heights on very short windows; remainder rows fall to
    // the last box.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(area);

    let session = app.selected_session();
    let plan_title = match session {
        Some(s) => format!(" Plan · {} ", s.name),
        None => " Plan ".to_string(),
    };

    // Build content per box; when there's no session yet, every box
    // shows a short empty-state hint so the column reads as 3 boxes
    // even on first launch.
    let (plan_lines, todos_lines, tasks_lines) = match session {
        Some(session) => {
            let state = app.agent_tasks.get(&session.id);
            let inner_width = rows[0].width.saturating_sub(2) as usize;
            let plan = build_plan_lines(state, inner_width, p);
            let todos = build_todos_lines(state, inner_width, p);
            let tasks = build_tasks_lines(state, inner_width, p);
            (
                or_empty_hint(plan, "Waiting for /plan…", p),
                or_empty_hint(todos, "No todos yet.", p),
                or_empty_hint(tasks, "No subagent runs yet.", p),
            )
        }
        None => {
            let hint = |s: &str| {
                vec![Line::from(Span::styled(
                    s.to_string(),
                    Style::default().fg(p.muted),
                ))]
            };
            (
                hint("Select a session to see its plan."),
                hint("Select a session to see its todos."),
                hint("Select a session to see its subagents."),
            )
        }
    };

    render_section(f, rows[0], &plan_title, plan_lines, p);
    render_section(f, rows[1], " Todos ", todos_lines, p);
    render_section(f, rows[2], " Agents ", tasks_lines, p);
}

fn or_empty_hint(
    lines: Vec<Line<'static>>,
    hint: &str,
    p: &Palette,
) -> Vec<Line<'static>> {
    if lines.is_empty() {
        vec![Line::from(Span::styled(
            hint.to_string(),
            Style::default().fg(p.muted),
        ))]
    } else {
        lines
    }
}

fn render_section(
    f: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    p: &Palette,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let block = Block::default()
        .title(Span::styled(
            title.to_string(),
            Style::default().fg(p.fg_strong).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.idle_border).bg(p.panel_bg))
        .style(Style::default().bg(p.panel_bg).fg(p.fg));
    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn build_plan_lines(
    state: Option<&AgentTaskState>,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    let Some(plan) = state.and_then(|s| s.plan.as_deref()) else {
        return Vec::new();
    };
    let inner = width.saturating_sub(2).max(8);
    plan.lines()
        .take(20)
        .map(|raw| {
            Line::from(Span::styled(
                truncate(raw, inner),
                Style::default().fg(p.fg),
            ))
        })
        .collect()
}

fn build_todos_lines(
    state: Option<&AgentTaskState>,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    let Some(state) = state else {
        return Vec::new();
    };
    if state.todos.is_empty() {
        return Vec::new();
    }
    let inner = width.saturating_sub(2).max(8);
    let mut out: Vec<Line<'static>> = Vec::new();

    let total = state.todos.len();
    let done = state
        .todos
        .iter()
        .filter(|t| matches!(t.status, TodoStatus::Completed))
        .count();
    out.push(Line::from(Span::styled(
        format!("  {done}/{total} done"),
        Style::default().fg(p.muted),
    )));

    for t in state.todos.iter().take(20) {
        let (badge, color) = match t.status {
            TodoStatus::Completed => ("[x]", p.success),
            TodoStatus::InProgress => ("[~]", p.warning),
            TodoStatus::Pending => ("[ ]", p.muted),
        };
        let label = match t.status {
            TodoStatus::InProgress => t.active_form.clone().unwrap_or_else(|| t.content.clone()),
            _ => t.content.clone(),
        };
        let text = format!("{badge} {label}");
        out.push(Line::from(Span::styled(
            truncate(&text, inner),
            Style::default().fg(color),
        )));
    }
    if state.todos.len() > 20 {
        out.push(Line::from(Span::styled(
            format!("  +{} more", state.todos.len() - 20),
            Style::default().fg(p.muted),
        )));
    }
    out
}

fn build_tasks_lines(
    state: Option<&AgentTaskState>,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    let Some(state) = state else {
        return Vec::new();
    };
    if state.tasks.is_empty() {
        return Vec::new();
    }
    let inner = width.saturating_sub(2).max(8);
    let mut out: Vec<Line<'static>> = Vec::new();
    // Newest tasks tend to be most interesting — show the tail.
    let take = 10usize;
    let skip = state.tasks.len().saturating_sub(take);
    for t in state.tasks.iter().skip(skip) {
        let (badge, color) = match t.status {
            TaskStatus::Running => ("●", p.warning),
            TaskStatus::Completed => ("✓", p.success),
            TaskStatus::Failed => ("✗", p.error),
        };
        let dur = match t.duration_ms {
            Some(ms) if ms >= 1000 => format!(" ({:.1}s)", ms as f64 / 1000.0),
            Some(ms) => format!(" ({ms}ms)"),
            None => String::new(),
        };
        let agent = t
            .subagent_type
            .as_deref()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        let text = format!("{badge} {}{agent}{dur}", t.description);
        out.push(Line::from(Span::styled(
            truncate(&text, inner),
            Style::default().fg(color),
        )));
    }
    if state.tasks.len() > take {
        out.insert(
            0,
            Line::from(Span::styled(
                format!("  +{} earlier", state.tasks.len() - take),
                Style::default().fg(p.muted),
            )),
        );
    }
    out
}
