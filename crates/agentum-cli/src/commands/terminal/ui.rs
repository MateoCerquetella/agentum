//! Pure draw functions for the terminal dashboard. No mutation, no IO.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use tui_term::widget::PseudoTerminal;

use std::collections::BTreeMap;
use std::time::SystemTime;

use agentum_core::Status as SessionStatus;
use agentum_core::transcript::{AgentTaskState, TaskStatus, TodoStatus};

use uuid::Uuid;

use super::api::ClaudeUsage;
use super::app::{
    AddHostField, AddHostForm, AddProfileField, AddProfileForm, App, ConnState, DirPickerState,
    ErrorEntry, Focus, GoalForm, HostAuthChoice, HostsOverlay, NewSessionField, NewSessionForm,
    NotifKind, Notification, Overlay, PendingAction, ProfilesOverlay, RenameState, Row,
    SettingsRow, SettingsState, Side, ToolPickerState, TreeSection, palette_catalog, status_dot,
    tool_icon,
};
use super::extensions::{self, Extension, LAZYGIT};
use super::iometer::{fmt_bytes, fmt_rate};
use super::prefs::{Prefs, SoundKind, StatusChip};
use super::theme::Palette;

#[derive(Clone, Copy)]
pub struct Areas {
    pub title: Rect,
    /// Sticky strip between title and body, shown when the events-bus
    /// WS has failed to reconnect ≥ 2 times. Zero-sized when hidden so
    /// the body claims the row back without a re-layout.
    pub banner: Rect,
    pub tree: Rect,
    /// Bottom slice of the tree column showing per-agent + per-session
    /// usage (tokens, context %, cost). Zero-sized when the tree column
    /// is hidden (fullscreen, sidebar collapsed) or too short to give
    /// the session list ≥ 8 rows after carving off the panel.
    pub usage: Rect,
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

#[allow(clippy::too_many_arguments)]
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
    show_banner: bool,
) -> Areas {
    // Right panel is suppressed in fullscreen and on terminals too
    // narrow to host it without crushing the terminal area. It stays
    // visible alongside lazygit so the agent's plan/todos/tasks remain
    // in view while the user works in git.
    let show_right =
        right_panel_visible && !fullscreen && area.width >= RIGHT_PANEL_MIN_TOTAL_WIDTH;

    // Fullscreen: drop the title row, tree column, and status row so the
    // active panes consume every available cell. The empty Rects keep the
    // draw_* helpers no-op (they short-circuit on `area.width == 0`).
    if fullscreen {
        let (terminal_rect, lazygit_rect) = split_main(area, lazygit_open, lazygit_width);
        let (term_left, term_right) = split_terminal(terminal_rect, split_open, term_split_pct);
        let empty = Rect {
            x: area.x,
            y: area.y,
            width: 0,
            height: 0,
        };
        return Areas {
            title: empty,
            banner: empty,
            tree: empty,
            usage: empty,
            terminal: term_left,
            terminal_right: term_right,
            lazygit: lazygit_rect,
            agent_tasks: None,
            status: empty,
        };
    }

    // Banner sits between title and body. Height 0 when hidden so the
    // body reclaims the row instead of a re-layout flicker each time the
    // ws bus flips state.
    let banner_h: u16 = if show_banner { 1 } else { 0 };
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(banner_h),
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
        let max_tree = v[2].width.saturating_sub(20);
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
    let lw = if lw_target == 0 || v[2].width.saturating_sub(tw).saturating_sub(lw_target) < 20 {
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
        && v[2]
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
        .split(v[2]);

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

    // Carve a fixed 10-row Usage panel off the bottom of the tree column.
    // Floor is 18 rows so the session list above keeps at least 8 rows;
    // tighter viewports hide the panel rather than starve the list.
    let tree_full = body[0];
    let usage_h: u16 = if tree_full.height >= 18 { 10 } else { 0 };
    let (tree_rect, usage_rect) = if usage_h > 0 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(usage_h)])
            .split(tree_full);
        (rows[0], rows[1])
    } else {
        (
            tree_full,
            Rect {
                x: tree_full.x,
                y: tree_full.y,
                width: 0,
                height: 0,
            },
        )
    };

    Areas {
        title: v[0],
        banner: v[1],
        tree: tree_rect,
        usage: usage_rect,
        terminal: term_left,
        terminal_right: term_right,
        lazygit: lazygit_rect,
        agent_tasks: agent_tasks_rect,
        status: v[3],
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
fn split_terminal(area: Rect, split_open: bool, term_split_pct: u16) -> (Rect, Option<Rect>) {
    if !split_open {
        return (area, None);
    }
    let pct = term_split_pct.clamp(TERM_SPLIT_MIN_PCT, TERM_SPLIT_MAX_PCT);
    if area.width >= 80 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(pct),
                Constraint::Percentage(100 - pct),
            ])
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

/// True when the connection has stayed bad long enough to be worth
/// telling the user about — either the events-bus WS has failed at
/// least its first retry, the WS is fully Disconnected, or the
/// periodic HTTP poll has missed twice in a row. The 2-failure floor
/// (on both axes) debounces a typical sub-second blip so the banner
/// doesn't flicker on every reconnect cycle.
pub fn should_show_reconnect_ui(app: &App) -> bool {
    if !app.was_connected {
        return false;
    }
    if app.http_fail_count >= 2 {
        return true;
    }
    match app.conn {
        ConnState::Connected | ConnState::Connecting => false,
        ConnState::Disconnected => true,
        ConnState::Reconnecting { attempt, .. } => attempt >= 2,
    }
}

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let show_reconnect = should_show_reconnect_ui(app);
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
        show_reconnect,
    );
    let p = &app.theme.palette;

    // Paint the body background across the entire frame so the void around
    // panels takes the theme colour, not the host terminal's default.
    let body = Block::default().style(Style::default().bg(p.body_bg).fg(p.fg));
    f.render_widget(body, f.area());

    if areas.title.height > 0 {
        draw_title(f, areas.title, app, p);
    }
    if areas.banner.height > 0 {
        draw_reconnect_banner(f, areas.banner, app, p);
    }
    if areas.tree.width > 0 {
        draw_tree(f, areas.tree, app, p);
    }
    if areas.usage.width > 0 && areas.usage.height > 0 {
        draw_usage_panel(f, areas.usage, app, p);
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
        // Hint strip: one-cell row above the status bar showing the bound
        // card id + truncated title. Rendered as an overlay so it doesn't
        // require a layout slot — the terminal pane is shortened by 1 row
        // only when the hint is active (Phase 2, plan 05).
        if let Some(hint) = app.hint_card.as_ref() {
            if areas.status.y > 0 {
                let hint_rect = Rect {
                    x: areas.status.x,
                    y: areas.status.y - 1,
                    width: areas.status.width,
                    height: 1,
                };
                let hint_text = format!(" card #{} — {} ", hint.card_id, hint.title);
                let hint_para =
                    Paragraph::new(hint_text).style(Style::default().fg(p.fg).bg(p.surface_bg));
                f.render_widget(hint_para, hint_rect);
            }
        }
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
            draw_new_session_overlay(f, f.area(), form, &app.hosts, tool_unavailable, p)
        }
        Overlay::Confirm(action) => draw_confirm_overlay(f, f.area(), action, p),
        Overlay::Settings(state) => draw_settings_overlay(f, f.area(), state, &app.prefs, p),
        Overlay::Rename(state) => draw_rename_overlay(f, f.area(), state, p),
        Overlay::Profiles(state) => draw_profiles_overlay(
            f,
            f.area(),
            state,
            app.active_profile.as_deref(),
            app.show_all_servers,
            p,
        ),
        Overlay::Goal(form) => draw_overlay_goal(f, f.area(), form, p),
        Overlay::Hosts(state) => {
            draw_hosts_overlay(f, f.area(), state, &app.hosts, &app.host_readiness_cache, p)
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
    // Server suffix surfaces the active profile so users juggling
    // multiple agentum servers (local + VPS) can see which one drives
    // the current pane without opening Ctrl-S. Hidden when no profile
    // is active (loopback, ad-hoc `--api`) to keep the bar tidy.
    let server_suffix = match app.active_profile.as_deref() {
        Some(name) => format!(" · @{name}"),
        None => String::new(),
    };
    let title = match app.selected_session() {
        Some(s) => format!(" agentum · {}{server_suffix} ", s.name),
        None => format!(" agentum · no session selected{server_suffix} "),
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
        let para =
            Paragraph::new(Line::from(vec![title_span])).style(Style::default().bg(p.body_bg));
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

    let title_para =
        Paragraph::new(Line::from(vec![title_span])).style(Style::default().bg(p.body_bg));
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

    // Hosts section leads the sidebar as a compact status strip: the
    // local machine first, then each SSH host the daemon drives. The
    // header shows a `▶ N` chip when collapsed. Cursors map 1:1 onto
    // `app.hosts`.
    let server_count = app.servers_row_count();
    let header_label = if app.servers_collapsed {
        format!(" HOSTS  ▶ {server_count}  (Ctrl-K V)")
    } else {
        " HOSTS".to_string()
    };
    items.push(ListItem::new(Line::from(Span::styled(
        header_label,
        Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
    ))));

    if !app.servers_collapsed {
        for (i, host) in app.hosts.iter().enumerate() {
            let is_cursor = app.tree_section == TreeSection::Servers && i == app.servers_cursor;
            let row_style = if is_cursor {
                Style::default()
                    .bg(p.cursor_bg)
                    .fg(if focused { p.cursor_fg } else { p.fg })
            } else {
                Style::default().bg(p.panel_bg).fg(p.fg)
            };
            let is_local = matches!(host.kind, agentum_core::HostKind::Local);
            // Health dot + trailing status. The local host is the daemon's
            // own machine (always live). SSH hosts read from the readiness
            // cache: green when the last check passed, amber "needs setup"
            // when it didn't, muted when never checked (Enter/`t` checks).
            let reachable = app.host_readiness_cache.get(&host.id).map(|(_, r)| r.ok);
            let (dot_color, trailing) = if is_local {
                (p.success, None)
            } else {
                match reachable {
                    Some(true) => (p.success, None),
                    Some(false) => (p.warning, Some(("  needs setup", p.warning))),
                    None => (p.muted, Some(("  press Enter to check", p.muted))),
                }
            };
            let label = if is_local {
                super::app::local_machine_label()
            } else {
                host.name.clone()
            };
            let mut spans = vec![
                Span::raw("   "),
                Span::styled("●", Style::default().fg(dot_color)),
                Span::raw(" "),
                Span::styled(
                    label,
                    Style::default()
                        .fg(if is_cursor && focused {
                            p.cursor_fg
                        } else {
                            p.fg_strong
                        })
                        .add_modifier(if is_cursor {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ];
            // SSH hosts show user@hostname as a dim hint so the row is
            // identifiable without opening the detail pane.
            if let agentum_core::HostKind::Ssh { user, hostname, .. } = &host.kind {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("{user}@{hostname}"),
                    Style::default().fg(p.muted),
                ));
            }
            if let Some((text, color)) = trailing {
                spans.push(Span::styled(text.to_string(), Style::default().fg(color)));
            }
            items.push(ListItem::new(Line::from(spans)).style(row_style));
        }
    }

    // Sessions section sits at the bottom — it's the scrollable list
    // that grows with the workload, anchored beneath the SERVERS
    // status strip.
    items.push(ListItem::new(Line::from(Span::styled(
        " SESSIONS".to_string(),
        Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
    ))));

    let cursor = app.tree.cursor;
    let in_sessions = app.tree_section == TreeSection::Sessions;
    for (i, row) in app.tree.rows().iter().enumerate() {
        let is_cursor = in_sessions && i == cursor;
        items.push(render_tree_row(app, *row, is_cursor, focused, p));
    }

    if app.tree.rows().is_empty() {
        let hint = if !filter.is_empty() {
            format!("   (no matches for ⌕{filter})")
        } else {
            "   (no sessions — press n)".to_string()
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
            // Server header — top-level row. The loopback row reads as
            // the host's own hostname (rendered in `fg_strong`) and
            // named profiles read as `@<name>` (rendered in
            // `accent_alt`). Same weight on both rows — the colour
            // alone carries the "this one is local" signal so the
            // sidebar doesn't shout at the user (an earlier iteration
            // bolded `MY MACHINE (linux)` in caps and got flagged as
            // aggressive).
            let label = super::app::profile_label(&g.profile);
            let count: usize = g.projects.iter().map(|pr| pr.sessions.len()).sum();
            let label_color = if g.profile.is_empty() {
                p.fg_strong
            } else {
                p.accent_alt
            };
            let spans = vec![
                Span::raw(format!(" {arrow} ")),
                Span::styled(label, Style::default().fg(label_color)),
                Span::styled(format!("  ({count})"), Style::default().fg(p.muted)),
            ];
            ListItem::new(Line::from(spans)).style(row_style)
        }
        Row::Project { group, project } => {
            let proj = &app.tree.groups[group].projects[project];
            let arrow = if proj.expanded { "▾" } else { "▸" };
            // Project header — indented under its server. Reads as the
            // workdir basename (`agentum`, `mc-site`, …) so the user
            // sees project identity without scanning a full absolute
            // path; the trailing count keeps the "how busy is this
            // project" signal that the v0.7.19 flat list lost.
            let label = super::app::workdir_label(&proj.workdir);
            let count = proj.sessions.len();
            let spans = vec![
                Span::raw(format!("   {arrow} ")),
                Span::styled(label, Style::default().fg(p.fg)),
                Span::styled(format!("  ({count})"), Style::default().fg(p.muted)),
            ];
            ListItem::new(Line::from(spans)).style(row_style)
        }
        Row::Leaf {
            group,
            project,
            leaf,
        } => {
            let id = app.tree.groups[group].projects[project].sessions[leaf];
            let checked = app.checked.contains(&id);
            let session = app.sessions.iter().find(|s| s.id == id);
            let (name, dot, dot_color, icon_glyph, icon_color, tool_label) = match session {
                Some(s) => {
                    // Priority: Crashed > Awaiting > Idle > Working >
                    // underlying status. A dead pane should never look
                    // like it's just waiting; a pending prompt overrides
                    // the green/idle dot so attention is unmissable; a
                    // sleeping agent reads as muted `◌` instead of green
                    // `●` so "working" and "idle at prompt" are visually
                    // distinct.
                    //
                    // Green `●` requires *positive evidence* the agent
                    // is working (`app.working`). The old fallback
                    // `status_dot(Status::Running)` returned green for
                    // every running tmux pane, which made every session
                    // whose connect-time replay snapshot was missing
                    // read as a misleading pulsing green
                    // (#stuck-green-dot regression).
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
                    } else if app.working.contains(&s.id)
                        && s.status == agentum_core::Status::Running
                    {
                        status_dot(s.status)
                    } else if s.status == agentum_core::Status::Running {
                        // Running tmux pane, agent state unknown (no
                        // agent.* observation yet, or replay snapshot
                        // was empty). Neutral dot instead of green —
                        // the watchdog will emit `agent.working` within
                        // ~1 s if the agent is actually active.
                        ("●", p.muted)
                    } else {
                        status_dot(s.status)
                    };
                    let (icon, icon_c) = tool_icon(&s.tool);
                    // Trailing label is just the model now — the agent
                    // identity moved to the leading colored icon, so
                    // repeating `claude/` ahead of the model is noise.
                    // Sessions with no model (raw shells, agents that
                    // didn't surface one) get an empty label so the
                    // trailing space doesn't read as a hanging artifact.
                    let trailing = s.model.clone().unwrap_or_default();
                    (s.name.clone(), dot, color, icon, icon_c, trailing)
                }
                None => ("?".into(), "?", p.error, "▣", p.muted, String::new()),
            };
            // Reserve a 4-cell prefix so checked/unchecked rows align.
            // `[x] ` (4 cells) when in the multi-select set; same width
            // of spaces otherwise. Coloured with `accent` so a checked
            // session is unmistakable against a long tree. The extra
            // leading "   " puts the leaf two indent steps deep — one
            // for the server header, one for the project header — so
            // the three-level hierarchy reads visually.
            let check_span = if checked {
                Span::styled(
                    "[x] ".to_string(),
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("    ")
            };
            // Per-agent icon sits BEFORE the name so the user scans
            // tool identity vertically down the sidebar — yellow `✻`
            // rows are Claude, green `❋` rows are Codex, etc. Two
            // leading spaces (instead of the original five) make room
            // for `icon + space` while keeping the visual indent of
            // a leaf at roughly the same column it was before.
            let mut spans = vec![
                Span::raw("   "),
                Span::styled(icon_glyph, Style::default().fg(icon_color)),
                Span::raw(" "),
                check_span,
                Span::raw(format!("{:<14}", truncate(&name, 14))),
                Span::raw(" "),
                Span::styled(dot, Style::default().fg(dot_color)),
                Span::raw(" "),
                Span::styled(tool_label, Style::default().fg(p.muted)),
            ];
            if is_cursor {
                // index 4 = the name span — bold it so the cursor row's
                // identity reads. (Shifted from 2 to 4 by the new
                // leading icon + space spans.)
                spans[4].style = Style::default().add_modifier(Modifier::BOLD);
            }
            ListItem::new(Line::from(spans)).style(row_style)
        }
    }
}

fn draw_terminal(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    // Servers section takes over the main pane: a session terminal is
    // meaningless here and the user wants to see who they're connected
    // to plus reconnect status. Mirrors how lazygit / split-right
    // pre-empt this slot — the pane belongs to whichever surface owns
    // the cursor.
    if app.tree_section == TreeSection::Servers {
        draw_servers_panel(f, area, app, p);
        return;
    }
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
        format!(
            "{} ↑ scroll {} ",
            base.trim_end(),
            app.term.scrollback_offset()
        )
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
        format!(
            "{} ↑ scroll {} ",
            base.trim_end(),
            slot.term.scrollback_offset()
        )
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

/// Detail card that replaces the terminal pane while the user's cursor
/// is on the SERVERS section. Shows the cursor-selected server's URL,
/// status, fingerprint, default/active markers, and last connection
/// error — plus a hint row for the keybindings the Servers section
/// owns. The status dot at the top mirrors the sidebar's dot (incl.
/// the spinner overlay while a reconnect is in flight).
fn draw_servers_panel(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let title = " hosts · Enter manage · a add · d remove ";
    // The Tree pane owns the cursor here; reflect that in the focus
    // styling so the user sees which sidebar section is active.
    let focused = app.focus == Focus::Tree;
    let block = panel_block(title, focused, p);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(host) = app.cursor_host() else {
        let hint =
            Paragraph::new("no host selected").style(Style::default().fg(p.muted).bg(p.panel_bg));
        f.render_widget(hint, inner);
        return;
    };

    let label_w = 14usize;
    let field = |k: &str, value: String, value_style: Style| -> Line<'static> {
        let key = Span::styled(
            format!(" {:<width$}", k, width = label_w),
            Style::default().fg(p.muted),
        );
        Line::from(vec![key, Span::styled(value, value_style)])
    };

    let is_local = matches!(host.kind, agentum_core::HostKind::Local);
    let readiness = app.host_readiness_cache.get(&host.id).map(|(_, r)| r);
    // Local is always live (it's the daemon's own machine). SSH hosts
    // read from the readiness cache: ready / needs-setup / not-checked.
    let (dot_color, status_text, status_color) = if is_local {
        (p.success, "live", p.success)
    } else {
        match readiness {
            Some(r) if r.ok => (p.success, "ready", p.success),
            Some(_) => (p.warning, "needs setup", p.warning),
            None => (p.muted, "not checked", p.muted),
        }
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(""));
    let label = if is_local {
        super::app::local_machine_label()
    } else {
        host.name.clone()
    };
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled("●", Style::default().fg(dot_color)),
        Span::raw("  "),
        Span::styled(
            label,
            Style::default()
                .fg(p.fg_strong)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(field(
        "status",
        status_text.to_string(),
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD),
    ));
    match &host.kind {
        agentum_core::HostKind::Local => {
            lines.push(field(
                "kind",
                "this machine".to_string(),
                Style::default().fg(p.fg),
            ));
        }
        agentum_core::HostKind::Ssh {
            user,
            hostname,
            port,
            ..
        } => {
            lines.push(field(
                "ssh",
                format!("{user}@{hostname}:{port}"),
                Style::default().fg(p.fg),
            ));
        }
    }
    if !is_local {
        lines.push(Line::from(""));
        match readiness {
            Some(r) => lines.push(field("check", r.message.clone(), Style::default().fg(p.fg))),
            None => lines.push(Line::from(Span::styled(
                "   press Enter to check readiness (deps + agents)",
                Style::default().fg(p.muted),
            ))),
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(""));
    let total = app.hosts.len();
    lines.push(Line::from(Span::styled(
        format!(
            " {} host{} configured",
            total,
            if total == 1 { "" } else { "s" }
        ),
        Style::default().fg(p.muted),
    )));
    lines.push(Line::from(Span::styled(
        " j/k move · Enter manage · a add · Ctrl-D remove".to_string(),
        Style::default().fg(p.muted),
    )));
    lines.push(Line::from(Span::styled(
        " l / → jump to this host's sessions".to_string(),
        Style::default().fg(p.muted),
    )));

    let para = Paragraph::new(lines)
        .style(Style::default().bg(p.panel_bg).fg(p.fg))
        .wrap(Wrap { trim: false });
    f.render_widget(para, inner);
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

    // "GOAL · planning…" chip: shown left-aligned while a goal is being
    // submitted to the planner. UI-SPEC: accent_alt (--cta) color so it
    // reads as an active CTA, not a passive informational label.
    if let Overlay::Goal(ref form) = app.overlay {
        if form.submitting {
            left.push(Span::styled(
                " GOAL \u{00b7} planning\u{2026} ",
                Style::default().fg(p.accent_alt).bg(p.chrome_bg),
            ));
        }
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

    // Bound-card chip: shown when the selected session has a card_id.
    // Pressing `c` in Focus::Tree toggles the one-cell hint strip.
    // Palette-only colors — no hardcoded Color::* (Phase 2, plan 05).
    if app.focus == Focus::Tree {
        if let Some(sess) = app.selected_session() {
            if let Some(card_id) = sess.card_id {
                right.push(Span::styled(
                    format!(" c card #{card_id} "),
                    Style::default().fg(p.muted).bg(p.chrome_bg),
                ));
            }
        }
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
        lines.push(Line::from(Span::styled(body, Style::default().fg(p.muted))));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(p.surface_bg));
    Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
}

fn draw_help_overlay(f: &mut Frame<'_>, area: Rect, lazygit_open: bool, p: &Palette) {
    let mut lines = vec![
        head("agentum terminal — keys", p),
        Line::from(""),
        head("  Universal (work even inside the terminal pane)", p),
        body("  Ctrl-P / Ctrl-Shift-P  command palette", p),
        body("  Ctrl-S            open the servers switcher overlay", p),
        body(
            "  Ctrl-H            SSH hosts readiness overlay (tree only)",
            p,
        ),
        body("  Ctrl-E            toggle focus: tree ↔ terminal", p),
        body("  Ctrl-G            toggle lazygit side pane", p),
        body("  Ctrl-Tab          flip back to last session", p),
        body("  Ctrl-B            toggle the sidebar tree", p),
        body(
            "  Ctrl-T            toggle the agent plan/todo/task panel",
            p,
        ),
        body("  Ctrl-K Z          toggle fullscreen (zen)", p),
        body("  Ctrl-K , / .      shrink / grow lazygit width", p),
        body("  Ctrl-\\            split the focused terminal pane", p),
        body("  Ctrl-W            close the split", p),
        body(
            "  Ctrl-Shift-←/→    resize the split divider (when split is open)",
            p,
        ),
        body(
            "  Ctrl-,            settings (notifications · layout · status bar)",
            p,
        ),
        body(
            "  Ctrl-R            rename the highlighted session (tree only)",
            p,
        ),
        body("  Mouse wheel       scroll the pane under the cursor", p),
        body(
            "  Shift-PgUp/PgDn   scroll the focused pane (no mouse needed)",
            p,
        ),
        body("  F5                next panel", p),
        body("  F6                previous panel", p),
        body(
            "  Ctrl-1 … Ctrl-9   jump to Nth project group in the tree",
            p,
        ),
        body("  Ctrl-Q            quit", p),
        body("  Ctrl-C            interrupt focused pane (else quit)", p),
        Line::from(""),
        head("  Tree", p),
        body("  1 / 2 / 3         focus tree / terminal / lazygit", p),
        body("  Tab               next panel", p),
        body("  Shift-Tab         previous panel", p),
        body(
            "  Ctrl-F            filter sessions by name (Esc clears)",
            p,
        ),
        body("  j / k / ↑ / ↓     move selection", p),
        body("  h / l / ← / →     collapse / expand group", p),
        body(
            "  Space             select session and focus the terminal",
            p,
        ),
        body(
            "  Enter / Alt-Enter check / uncheck cursor row (Alt works from any focus)",
            p,
        ),
        body(
            "  u · s · K/x/D     act on checked set (else cursor row)",
            p,
        ),
        body("  Esc               clear checks · filter · fullscreen", p),
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
        body(
            "  s                 stop the selected session (graceful)",
            p,
        ),
        body(
            "  K · x · D         kill the selected session (closes & removes)",
            p,
        ),
        Line::from(""),
        head("  Board", p),
        body(
            "  G                 open goal composer (from tree focus)",
            p,
        ),
        body(
            "    Enter             add newline · Ctrl-Enter submit · Esc cancel",
            p,
        ),
        Line::from(""),
        head("  Extensions & appearance", p),
        body("  g                 toggle lazygit side pane", p),
        body("  G (lazygit open)  lazygit cheat sheet", p),
        body("  T                 cycle theme", p),
        body(
            "  Ctrl-P then ~     status bar settings (toggle each chip individually)",
            p,
        ),
        body(
            "  ↓ rate ↑ rate     live WS throughput · toggle via ~ I/O speeds",
            p,
        ),
        body(
            "  Shift-F           toggle fullscreen (hide tree + chrome)",
            p,
        ),
        body("  Esc               exit fullscreen", p),
        body(
            "  + / -             widen / narrow focused side column (tree, or lazygit when focused)",
            p,
        ),
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
        SettingsRow::SoundMaster => ("  Sound: master".into(), onoff(prefs.sound_master).into()),
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
            Style::default()
                .fg(p.fg_strong)
                .bg(p.chip_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        let value_style = if i == cursor {
            Style::default()
                .fg(p.accent)
                .bg(p.chip_bg)
                .add_modifier(Modifier::BOLD)
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
    show_all_servers: bool,
    p: &Palette,
) {
    // Two visual modes: a list of profiles, or the inline add-form.
    // Shared frame so the size doesn't jump between them.
    if let Some(form) = &state.add_form {
        draw_profiles_add_form(f, area, form, p);
        return;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head("Servers", p));
    // Tree-scope chip: surfaces the `show_all_servers` flag so users
    // know which mode the sidebar is in before they flip with `s`.
    // "all" is the recommended default; "active only" is the focus mode.
    let scope_label = if show_all_servers {
        "scope: all servers' sessions  (recommended)"
    } else {
        "scope: active server only"
    };
    lines.push(Line::from(Span::styled(
        format!("  {scope_label}"),
        Style::default().fg(p.muted),
    )));
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
        "  Enter switch · a add · e edit · d remove · s scope · Esc close".to_string(),
        Style::default().fg(p.muted),
    )));

    overlay_box(f, area, " servers ", lines, 80, p);
}

fn draw_profiles_add_form(f: &mut Frame<'_>, area: Rect, form: &AddProfileForm, p: &Palette) {
    let editing = form.editing.is_some();
    let (heading, frame_title) = if editing {
        ("Edit server", " edit server ")
    } else {
        ("Add server", " add server ")
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head(heading, p));
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
    if let Some(err) = form.error.as_ref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {err}"),
            Style::default().fg(p.error),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Tab next · Enter save · Esc back".to_string(),
        Style::default().fg(p.muted),
    )));
    overlay_box(f, area, frame_title, lines, 80, p);
}

/// Status dot for a host's last readiness report:
/// green = ready (all agents too), yellow = required ok but some agent
/// CLIs missing, red = a required dep missing / unreachable, hollow grey
/// = not checked yet.
fn host_dot_style(
    report: Option<&agentum_core::HostReadiness>,
    p: &Palette,
) -> (&'static str, Color) {
    match report {
        None => ("○", p.muted),
        Some(r) if !r.ok => ("●", p.error),
        Some(r) if r.agents.iter().any(|a| !a.installed) => ("●", p.warning),
        Some(_) => ("●", p.success),
    }
}

/// Render the Ctrl-H hosts overlay: a list of daemon-controlled hosts
/// with readiness dots, plus a detail pane for the selected host. Dots
/// and detail come from `cache`; a host with no cache entry shows a
/// hollow dot and the user presses Enter/`t` to probe it.
fn draw_hosts_overlay(
    f: &mut Frame<'_>,
    area: Rect,
    state: &HostsOverlay,
    hosts: &[agentum_core::Host],
    cache: &std::collections::HashMap<Uuid, (tokio::time::Instant, agentum_core::HostReadiness)>,
    p: &Palette,
) {
    // Add-host form takes over the overlay when active; its folder picker
    // (for the SSH key path) takes over in turn while it's open.
    if let Some(form) = &state.add_form {
        if let Some(picker) = &form.picker {
            draw_dir_picker_overlay(f, area, picker, p);
        } else {
            draw_hosts_add_form(f, area, form, p);
        }
        return;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head("SSH Hosts", p));
    lines.push(Line::from(""));

    if state.host_ids.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No SSH hosts yet.".to_string(),
            Style::default().fg(p.fg),
        )));
        lines.push(Line::from(Span::styled(
            "  Press `a` to add a server — agentum will SSH in, scan it,".to_string(),
            Style::default().fg(p.muted),
        )));
        lines.push(Line::from(Span::styled(
            "  install tmux + git, and ask which agent CLIs to install.".to_string(),
            Style::default().fg(p.muted),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  a add · Esc close".to_string(),
            Style::default().fg(p.muted),
        )));
        overlay_box(f, area, " hosts ", lines, 88, p);
        return;
    }

    if let Some(err) = state.error.as_ref() {
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {err}"),
            Style::default().fg(p.error),
        )));
        lines.push(Line::from(""));
    }

    // ── Host list ──────────────────────────────────────────────────
    for (i, id) in state.host_ids.iter().enumerate() {
        let Some(host) = hosts.iter().find(|h| &h.id == id) else {
            continue;
        };
        let report = cache.get(id).map(|(_, r)| r);
        let selected = i == state.cursor;
        let (dot, dot_color) = host_dot_style(report, p);
        let marker = if selected { "▶ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(
                marker.to_string(),
                Style::default().fg(if selected { p.accent } else { p.muted }),
            ),
            Span::styled(format!("{dot} "), Style::default().fg(dot_color)),
            Span::styled(
                host.name.clone(),
                Style::default()
                    .fg(if selected { p.fg_strong } else { p.fg })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", host_target_label(host)),
                Style::default().fg(p.muted),
            ),
        ]));
    }

    // ── Detail pane for the selected host ──────────────────────────
    lines.push(Line::from(""));
    let selected_report = state
        .selected()
        .and_then(|id| cache.get(&id).map(|(_, r)| r));
    if state.loading {
        lines.push(Line::from(Span::styled(
            "  checking… (one SSH round trip)".to_string(),
            Style::default().fg(p.muted),
        )));
    } else if let Some(r) = selected_report {
        let uname = r.system.uname.as_deref().unwrap_or("unknown");
        let sudo = match r.system.sudo_nopasswd {
            Some(true) => " · sudo: passwordless",
            Some(false) => " · sudo: password (bootstrap fails)",
            None => "",
        };
        lines.push(Line::from(Span::styled(
            format!("  {uname} · pkg={}{sudo}", r.system.pkg_manager),
            Style::default().fg(p.subtle),
        )));
        lines.push(Line::from(Span::styled(
            "  REQUIRED".to_string(),
            Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
        )));
        for dep in &r.required {
            push_dep_line(
                &mut lines,
                &dep.label,
                dep.installed,
                dep.install_hint.as_deref(),
                p,
            );
        }
        lines.push(Line::from(Span::styled(
            "  AGENTS (optional)".to_string(),
            Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
        )));
        for agent in &r.agents {
            push_dep_line(
                &mut lines,
                &agent.id,
                agent.installed,
                agent.install_hint.as_deref(),
                p,
            );
        }
        lines.push(Line::from(""));
        let (verdict, color) = if r.ok {
            ("Ready: yes".to_string(), p.success)
        } else if r.system.uname.is_none() {
            (format!("Ready: no — {}", r.message), p.error)
        } else {
            let missing = r.required.iter().filter(|d| !d.installed).count();
            (format!("Ready: no ({missing} required missing)"), p.error)
        };
        lines.push(Line::from(Span::styled(
            format!("  {verdict}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  press Enter/t to run a readiness check".to_string(),
            Style::default().fg(p.muted),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ move · Enter/t check · i set up (deps + agents) · a add · Esc close".to_string(),
        Style::default().fg(p.muted),
    )));

    overlay_box(f, area, " hosts ", lines, 88, p);
}

/// Render the add-host form (Ctrl-H → `a`). Collects the SSH fields plus an
/// auth toggle (key/agent or password). The password value is masked.
fn draw_hosts_add_form(f: &mut Frame<'_>, area: Rect, form: &AddHostForm, p: &Palette) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(head("Add an SSH host", p));
    lines.push(Line::from(""));
    push_form_field(
        &mut lines,
        "Name",
        &form.name,
        form.field == AddHostField::Name,
        "omarchy",
        p,
    );
    push_form_field(
        &mut lines,
        "User",
        &form.user,
        form.field == AddHostField::User,
        "me",
        p,
    );
    push_form_field(
        &mut lines,
        "Hostname",
        &form.hostname,
        form.field == AddHostField::Hostname,
        "omarchy.local",
        p,
    );
    push_form_field(
        &mut lines,
        "Port",
        &form.port,
        form.field == AddHostField::Port,
        "22",
        p,
    );

    // Auth toggle row: highlight the active choice; Space/←→ flips it.
    let auth_focused = form.field == AddHostField::Auth;
    let label_color = if auth_focused { p.accent } else { p.muted };
    lines.push(Line::from(vec![
        Span::styled(
            "  Auth".to_string(),
            Style::default()
                .fg(label_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  (Space / ←→ to switch)".to_string(),
            Style::default().fg(p.muted),
        ),
    ]));
    let key_sel = form.auth == HostAuthChoice::Key;
    let pw_sel = form.auth == HostAuthChoice::Password;
    let pick = |on: bool, text: &str| {
        if on {
            Span::styled(
                format!("[{text}]"),
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {text} "), Style::default().fg(p.muted))
        }
    };
    lines.push(Line::from(vec![
        Span::raw("    "),
        pick(key_sel, "key/agent"),
        Span::raw("  "),
        pick(pw_sel, "password"),
    ]));

    // Secret field: masked when collecting a password.
    let secret_focused = form.field == AddHostField::Secret;
    let display = match form.auth {
        HostAuthChoice::Password => "•".repeat(form.secret.chars().count()),
        HostAuthChoice::Key => form.secret.clone(),
    };
    let placeholder = match form.auth {
        HostAuthChoice::Key => "~/.ssh/id_ed25519 (blank = ssh-agent)",
        HostAuthChoice::Password => "password",
    };
    // In key mode the secret is a path, so offer the same folder-picker
    // gesture as the New Session workdir field. Password mode has no file.
    let secret_hint = match form.auth {
        HostAuthChoice::Key => Some("Enter opens the folder picker"),
        HostAuthChoice::Password => None,
    };
    push_form_field_with_hint(
        &mut lines,
        form.secret_label(),
        &display,
        secret_focused,
        placeholder,
        secret_hint,
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
    let footer = if form.submitting {
        "  saving…".to_string()
    } else {
        "  Tab next · Enter save & scan · Esc back".to_string()
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(p.muted),
    )));
    overlay_box(f, area, " add host ", lines, 80, p);
}

/// Push one `[x]/[ ] label — hint` dependency row into a line buffer,
/// colouring the mark green/red and showing the install hint only when
/// the dep is missing.
fn push_dep_line(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    installed: bool,
    hint: Option<&str>,
    p: &Palette,
) {
    let (mark, color) = if installed {
        ("✓", p.success)
    } else {
        ("✗", p.error)
    };
    let mut spans = vec![
        Span::styled(format!("    {mark} "), Style::default().fg(color)),
        Span::styled(format!("{label:<10}"), Style::default().fg(p.fg)),
    ];
    if !installed && let Some(h) = hint {
        spans.push(Span::styled(
            format!(" — {h}"),
            Style::default().fg(p.muted),
        ));
    }
    lines.push(Line::from(spans));
}

/// `ssh user@host:port` / `this machine` label for a host row.
fn host_target_label(host: &agentum_core::Host) -> String {
    match &host.kind {
        agentum_core::HostKind::Local => "this machine".to_string(),
        agentum_core::HostKind::Ssh {
            user,
            hostname,
            port,
            ..
        } => format!("ssh {user}@{hostname}:{port}"),
    }
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
        text = text
            .chars()
            .take(text_w.saturating_sub(1))
            .collect::<String>()
            + "…";
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
    hosts: &[agentum_core::Host],
    tool_unavailable: bool,
    p: &Palette,
) {
    // If the directory picker is up, it owns the overlay box.
    if let Some(picker) = &form.picker {
        draw_dir_picker_overlay(f, area, picker, p);
        return;
    }
    // Tool picker is a sibling modal — same precedence rules as the
    // dir picker (whichever is open eats input until dismissed).
    if let Some(picker) = &form.tool_picker {
        draw_tool_picker_overlay(f, area, picker, p);
        return;
    }

    let mut lines: Vec<Line<'_>> = Vec::new();
    // Title used to be duplicated — once as the overlay-box border title
    // and once as a `head("New session", …)` line inside the box. We
    // keep the box title (set at the `overlay_box` call below) and drop
    // the inner heading so the form starts straight on its first field.

    // Servers pick comes first — same order as the user's mental
    // model ("which agentum, then which folder, then which agent").
    // The empty string renders as the local-machine label so the
    // loopback is a peer of any named remote profile in the cycle
    // (pre-v0.7.9 said "(current connection)" which a user reported
    // as confusing — they wanted the local case to look like just
    // another server entry).
    let local_label = super::app::local_machine_label();
    let profile_display = super::app::profile_label(form.profile.trim());
    push_form_field_with_hint(
        &mut lines,
        "Servers",
        &profile_display,
        form.field == NewSessionField::Profile,
        &local_label,
        Some("Tab cycles the local machine + configured servers"),
        p,
    );
    let host_display = if form.host_id.trim().is_empty() {
        "local".to_string()
    } else {
        hosts
            .iter()
            .find(|h| h.id.to_string() == form.host_id)
            .map(|h| h.name.clone())
            .unwrap_or_else(|| form.host_id.clone())
    };
    push_form_field_with_hint(
        &mut lines,
        "Host",
        &host_display,
        form.field == NewSessionField::Host,
        "local",
        Some("Tab cycles SSH hosts controlled by this server"),
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
            Some("Tab cycles · Enter opens the agent picker"),
            p,
        );
    }
    push_form_field_with_hint(
        &mut lines,
        "Model",
        &form.model,
        form.field == NewSessionField::Model,
        "e.g. claude-opus-4-8",
        Some("(optional)"),
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
    // Worktree isolation. Mirrors the dashboard's "Isolate in git
    // worktree" checkbox but defaults on. When a host is selected the
    // daemon can't honor it (SSH hosts have no worktree path yet), so
    // the label says so and the submit path drops the request.
    let worktree_label = if form.host_id.trim().is_empty() {
        "Isolate in git worktree (own branch + checkout)"
    } else {
        "Isolate in git worktree (local hosts only)"
    };
    push_toggle_field(
        &mut lines,
        worktree_label,
        form.use_worktree,
        form.field == NewSessionField::Worktree,
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
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(h) = hint {
        label_spans.push(Span::styled(format!("  {h}"), Style::default().fg(p.muted)));
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
            Style::default()
                .fg(label_color)
                .add_modifier(Modifier::BOLD),
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

/// Modal picker for the New-Session form's Tool field. Mirrors
/// `draw_dir_picker_overlay`'s shape (cursor row, hint footer) so
/// muscle memory transfers between the two pickers.
fn draw_tool_picker_overlay(f: &mut Frame<'_>, area: Rect, picker: &ToolPickerState, p: &Palette) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(head("Pick an agent", p));
    lines.push(Line::from(""));

    if picker.entries.is_empty() {
        // Cold-state safeguard: TOOL_SUGGESTIONS is non-empty in this
        // build, but reflecting `entries.is_empty()` keeps the overlay
        // useful if a future filter knocks every entry out.
        lines.push(Line::from(Span::styled(
            "  (no agents available)",
            Style::default().fg(p.muted),
        )));
    } else {
        // Show the full list — TOOL_SUGGESTIONS is bounded and short,
        // so we don't need the dir picker's 14-entry window.
        let name_width = picker
            .entries
            .iter()
            .map(|e| e.name.len())
            .max()
            .unwrap_or(0)
            .max(8);
        for (i, entry) in picker.entries.iter().enumerate() {
            let is_cursor = i == picker.cursor;
            let prefix = if is_cursor { "  > " } else { "    " };
            // Suffix mirrors the dashboard tile-dim: an uninstalled
            // probed agent is greyed out and tagged so the picker
            // reads at a glance.
            let avail_tag = if entry.available {
                ""
            } else {
                "  (not installed)"
            };
            let line_style = if is_cursor {
                Style::default().fg(p.cursor_fg).bg(p.cursor_bg)
            } else if entry.available {
                Style::default().fg(p.fg)
            } else {
                Style::default().fg(p.muted)
            };
            let text = format!(
                "{prefix}{name:<width$}  {desc}{tag}",
                name = entry.name,
                width = name_width,
                desc = entry.description,
                tag = avail_tag,
            );
            lines.push(Line::from(Span::styled(text, line_style)));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  ↑/↓", Style::default().fg(p.accent)),
        Span::styled(" move   ", Style::default().fg(p.muted)),
        Span::styled("Enter", Style::default().fg(p.accent)),
        Span::styled(" select   ", Style::default().fg(p.muted)),
        Span::styled("Esc", Style::default().fg(p.accent)),
        Span::styled(" back", Style::default().fg(p.muted)),
    ]));
    overlay_box(f, area, " Agent picker ", lines, 70, p);
}

fn draw_dir_picker_overlay(f: &mut Frame<'_>, area: Rect, picker: &DirPickerState, p: &Palette) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(head("Pick a working directory", p));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  current  ", Style::default().fg(p.muted)),
        Span::styled(
            picker.path.clone(),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ),
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

/// Slim 1-row strip just under the title. Shown for as long as the
/// events-bus WS hasn't reconnected (gated by `should_show_reconnect_ui`
/// so a quick blip doesn't flicker the layout). Mirrors the modal
/// overlay's wording so the user gets continuous reassurance without
/// the modal taking over content.
fn draw_reconnect_banner(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let dots = match app.tick_count % 4 {
        0 => "   ",
        1 => ".  ",
        2 => ".. ",
        _ => "...",
    };
    let label = match app.conn {
        ConnState::Reconnecting { attempt, delay_ms } => {
            let secs = delay_ms as f64 / 1000.0;
            format!(" ⟳ reconnecting · attempt {attempt} · retrying in {secs:.1}s{dots}")
        }
        ConnState::Disconnected => format!(" ✗ disconnected · reconnecting{dots}"),
        // WS is technically fine but HTTP polls keep failing — the
        // daemon may be hung or the TCP path went stale. Surface it
        // distinctly so the user knows the bus isn't the problem.
        ConnState::Connected | ConnState::Connecting => {
            format!(
                " ⚠ daemon not responding · {} HTTP failure{}{}",
                app.http_fail_count,
                if app.http_fail_count == 1 { "" } else { "s" },
                dots
            )
        }
    };
    let para = Paragraph::new(Line::from(Span::styled(
        label,
        Style::default()
            .fg(p.warning)
            .bg(p.chrome_bg)
            .add_modifier(Modifier::BOLD),
    )))
    .style(Style::default().bg(p.chrome_bg));
    f.render_widget(para, area);
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

fn or_empty_hint(lines: Vec<Line<'static>>, hint: &str, p: &Palette) -> Vec<Line<'static>> {
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
            Style::default()
                .fg(p.fg_strong)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.idle_border).bg(p.panel_bg))
        .style(Style::default().bg(p.panel_bg).fg(p.fg));
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
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

// ── Goal overlay ──────────────────────────────────────────────────────────────

/// Draw the Goal composer overlay.
///
/// Layout (top-to-bottom inside the overlay box):
/// - Empty line
/// - Placeholder text (muted) when the form is empty, else the typed text
///   with each line rendered separately so multi-line goals read naturally
/// - Empty line
/// - Footer: error in `palette.error` when present, else hint in `palette.muted`
///
/// The overlay width is 60 columns; height grows with the text, capped at
/// half the terminal height.
///
/// Colors follow the UI-SPEC palette mapping:
///   `--cta`     → `palette.accent_alt`   (GOAL chip, footer "Ctrl-Enter")
///   `--link`    → `palette.accent`       (footer key hints)
///   `--crash`   → `palette.error`        (error line)
///   `--fg-3`    → `palette.muted`        (placeholder, hint labels)
fn draw_overlay_goal(f: &mut Frame<'_>, area: Rect, form: &GoalForm, p: &Palette) {
    const OVERLAY_WIDTH: u16 = 60;
    const PLACEHOLDER: &str = "Drop a goal in. The planner will turn it into 3\u{2013}7 cards.";

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Empty spacer above the text area.
    lines.push(Line::from(""));

    if form.text.is_empty() {
        // Placeholder rendered in muted so it reads as a hint not real text.
        lines.push(Line::from(Span::styled(
            format!("  {PLACEHOLDER}"),
            Style::default().fg(p.muted),
        )));
    } else {
        // Render each line of the multi-line text separately. Trim a trailing
        // lone newline that the user may have added by accident — but preserve
        // internal newlines so intentional multi-paragraph goals show as typed.
        let text_to_show = form.text.trim_end_matches('\n');
        for raw_line in text_to_show.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {raw_line}"),
                Style::default().fg(p.fg),
            )));
        }
    }

    // Spacer before footer.
    lines.push(Line::from(""));

    // Footer: error or keyboard hint.
    if let Some(err) = &form.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(p.error),
        )));
    } else if form.submitting {
        lines.push(Line::from(Span::styled(
            "  planning\u{2026}",
            Style::default().fg(p.muted),
        )));
    } else {
        // Keyboard hint: "Enter newline · Ctrl-Enter to plan · Esc cancel"
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("Enter", Style::default().fg(p.accent)),
            Span::styled(" newline \u{00b7} ", Style::default().fg(p.muted)),
            Span::styled("Ctrl-Enter", Style::default().fg(p.accent_alt)),
            Span::styled(" to plan \u{00b7} ", Style::default().fg(p.muted)),
            Span::styled("Esc", Style::default().fg(p.accent)),
            Span::styled(" cancel", Style::default().fg(p.muted)),
        ]));
    }

    // Title chip: " GOAL " styled in accent_alt to match the --cta mapping.
    let title = " GOAL ";

    // Use the standard overlay_box helper so focus-border, surface-bg, and
    // centering are consistent with every other overlay in the TUI.
    overlay_box_with_title_style(f, area, title, lines, OVERLAY_WIDTH, p.accent_alt, p);
}

/// Variant of `overlay_box` that takes an explicit title color so the Goal
/// overlay's GOAL chip can use `accent_alt` (--cta) while other overlays
/// keep the default `accent` title color.
fn overlay_box_with_title_style(
    f: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: Vec<Line<'_>>,
    width: u16,
    title_color: Color,
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
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
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

// ---------------------------------------------------------------------------
// Usage panel — bottom-left readout of tokens / ctx / cost across running
// sessions. The four helpers below are pure (testable) so the render
// function stays a thin composition. The Session struct stores tokens as
// i64 and ctx as i32 (see crates/agentum-core/src/lib.rs); negative values
// are treated as missing so a stale "-1 sentinel" never bleeds onto screen.
// ---------------------------------------------------------------------------

/// Format a token count for the Usage panel. `None` and any negative value
/// render as an em-dash so the wire shape stays compact when the watchdog
/// hasn't reported tokens yet. Thresholds (`1_000`, `1_000_000`) match the
/// FleetRow convention so two surfaces never disagree on the same number.
pub(super) fn format_tokens(t: Option<i64>) -> String {
    match t {
        None => "—".to_string(),
        Some(n) if n < 0 => "—".to_string(),
        Some(n) if n < 1_000 => n.to_string(),
        Some(n) if n < 1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        Some(n) => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Format a USD cost. Non-finite (NaN/inf) and negative values collapse to
/// the em-dash sentinel — they only appear when an upstream pricing table
/// returns garbage, and rendering "$NaN" would be worse than admitting we
/// don't know.
pub(super) fn format_cost(c: Option<f64>) -> String {
    match c {
        None => "—".to_string(),
        Some(v) if !v.is_finite() || v < 0.0 => "—".to_string(),
        Some(v) => format!("${:.2}", v),
    }
}

/// Format a context-remaining percentage. The watchdog clamps to 0..=100,
/// but we still guard the range here so a future agent that overshoots
/// doesn't render "150%" without a fallback.
pub(super) fn format_ctx(p: Option<i32>) -> String {
    match p {
        None => "—".to_string(),
        Some(v) if !(0..=100).contains(&v) => "—".to_string(),
        Some(v) => format!("{}%", v),
    }
}

/// Truncate-or-pad a string to exactly `n` display chars. Truncation
/// appends `…` so the column boundary stays visible without ambiguity.
/// `n.saturating_sub(1)` guards the degenerate `n == 0` case (returns "").
fn truncate_pad(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len > n {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    } else {
        let mut out = s.to_string();
        for _ in len..n {
            out.push(' ');
        }
        out
    }
}

/// Color band for a plan-limit utilization %: 🟢 `<70`, 🟡 `70..=90`,
/// 🔴 `>90` (spec 001). Pure so the thresholds are unit-testable. The
/// `>90` band is strict — exactly 90 is still yellow — matching the
/// spec's `70–90` / `>90` boundary.
pub(super) fn band_color(pct: f64, p: &Palette) -> Color {
    if pct > 90.0 {
        p.error
    } else if pct >= 70.0 {
        p.warning
    } else {
        p.success
    }
}

/// Emoji glyph for the band, for the at-a-glance dot in the header.
fn band_glyph(pct: f64) -> &'static str {
    if pct > 90.0 {
        "🔴"
    } else if pct >= 70.0 {
        "🟡"
    } else {
        "🟢"
    }
}

/// Compact "resets in" label from a unix-ms reset time. `now_ms` is
/// passed in so the formatting stays pure/testable. Past or missing
/// times collapse to "now". Mirrors `format_short_age`'s grain.
fn format_resets_in(resets_at_ms: i64, now_ms: i64) -> String {
    let secs = ((resets_at_ms - now_ms) / 1000).max(0);
    if secs < 60 {
        "now".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Build the spec-001 usage header line for the Claude account readout.
/// Returns `None` (caller renders nothing extra) when there's no usage
/// snapshot at all. Otherwise yields a styled `Line`:
///   `est $12.40 · 2.1M tok · 🟡 82% · resets 2h`
/// When the plan-limit % is unavailable (no token / OAuth failed →
/// `source != "oauth"` or `limit_pct == None`), the band segment becomes
/// an explicit "usage unavailable" rather than a wrong number.
fn usage_header_line<'a>(usage: &ClaudeUsage, p: &Palette) -> Line<'a> {
    let now_ms = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let mut spans: Vec<Span> = Vec::new();

    // Estimated $ (labeled est.) — reuse the shared cost formatter.
    spans.push(Span::styled(
        format!("est {}", format_cost(usage.est_cost_usd)),
        Style::default().fg(p.fg),
    ));
    spans.push(Span::styled("  ", Style::default().fg(p.muted)));

    // Window tokens.
    spans.push(Span::styled(
        format!("{} tok", format_tokens(Some(usage.window_tokens as i64))),
        Style::default().fg(p.muted),
    ));

    // Plan-limit band — only a real number when source == oauth.
    let oauth = usage.source.as_deref() == Some("oauth");
    match (oauth, usage.limit_pct) {
        (true, Some(pct)) => {
            spans.push(Span::styled("  ", Style::default().fg(p.muted)));
            spans.push(Span::raw(band_glyph(pct)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("{:.0}%", pct),
                Style::default()
                    .fg(band_color(pct, p))
                    .add_modifier(Modifier::BOLD),
            ));
            if let Some(reset_ms) = usage.resets_at_ms {
                spans.push(Span::styled(
                    format!("  resets {}", format_resets_in(reset_ms, now_ms)),
                    Style::default().fg(p.muted),
                ));
            }
        }
        _ => {
            // Graceful degradation: no plan % to show. Be explicit so the
            // user never mistakes a blank for 0% headroom.
            spans.push(Span::styled(
                "  · plan usage unavailable",
                Style::default().fg(p.muted),
            ));
        }
    }

    Line::from(spans)
}

/// Bottom-left "Usage" panel. A spec-001 header line (Claude account
/// estimated $, window tokens, plan-limit band) sits above two stacked
/// sections: top half aggregates running sessions by tool name (count +
/// total tokens); bottom half lists each running session sorted by spend.
/// Passive readout — never focused — so we render with `panel_block(..,
/// false, ..)`. Short-circuits on zero-sized rects to match the rest of
/// the draw_* helpers.
pub(super) fn draw_usage_panel(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let block = panel_block(" Usage ", false, p);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Carve off a 1-row header for the account-wide usage readout when we
    // have a snapshot. This renders even with zero active agents — plan
    // headroom is account-wide, not per-session.
    let (header_area, inner) = if app.claude_usage.is_some() && inner.height >= 3 {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(inner);
        (Some(split[0]), split[1])
    } else {
        (None, inner)
    };
    if let (Some(hdr), Some(usage)) = (header_area, app.claude_usage.as_ref()) {
        let line = usage_header_line(usage, p);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(p.panel_bg)),
            hdr,
        );
    }

    // Collect once; both halves reuse this slice.
    let running: Vec<&agentum_core::Session> = app
        .sessions
        .iter()
        .filter(|s| s.status == SessionStatus::Running)
        .collect();

    if running.is_empty() {
        let empty = Paragraph::new("No active agents")
            .alignment(Alignment::Center)
            .style(Style::default().fg(p.muted).bg(p.panel_bg));
        f.render_widget(empty, inner);
        return;
    }

    let halves = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);
    let top_half = halves[0];
    let bottom_half = halves[1];

    // --- Top half: per-tool aggregate ---
    // BTreeMap keeps the by-tool iteration order stable so the alpha
    // tie-breaker below behaves deterministically across redraws.
    let mut by_tool: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    for s in &running {
        let entry = by_tool.entry(s.tool.clone()).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += s.tokens.unwrap_or(0).max(0);
    }
    let mut tool_rows: Vec<(String, usize, i64)> =
        by_tool.into_iter().map(|(k, v)| (k, v.0, v.1)).collect();
    // Count desc, then tool name asc — matches the dashboard's FleetRow ordering.
    tool_rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    if top_half.height > 0 {
        let top_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(top_half);
        let title = Paragraph::new("Agents")
            .style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD));
        f.render_widget(title, top_split[0]);

        let body_rows = top_split[1].height as usize;
        let items: Vec<ListItem> = tool_rows
            .iter()
            .take(body_rows)
            .map(|(tool, count, tokens)| {
                let line = Line::from(vec![
                    Span::styled(truncate_pad(tool, 8), Style::default().fg(p.fg_strong)),
                    Span::raw("  "),
                    Span::styled(format!("{} sess", count), Style::default().fg(p.fg)),
                    Span::raw("  "),
                    Span::styled(format_tokens(Some(*tokens)), Style::default().fg(p.muted)),
                ]);
                ListItem::new(line)
            })
            .collect();
        f.render_widget(List::new(items), top_split[1]);
    }

    // --- Bottom half: per-session detail, biggest spend first ---
    let mut by_session: Vec<&agentum_core::Session> = running.clone();
    by_session.sort_by(|a, b| b.tokens.unwrap_or(0).cmp(&a.tokens.unwrap_or(0)));

    if bottom_half.height > 0 {
        let bot_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(bottom_half);
        let title = Paragraph::new("Tasks")
            .style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD));
        f.render_widget(title, bot_split[0]);

        let body_rows = bot_split[1].height as usize;
        let items: Vec<ListItem> = by_session
            .iter()
            .take(body_rows)
            .map(|s| {
                let line = Line::from(vec![
                    Span::styled(truncate_pad(&s.name, 10), Style::default().fg(p.fg_strong)),
                    Span::raw("  "),
                    Span::styled(
                        truncate_pad(&format_ctx(s.ctx), 4),
                        Style::default().fg(p.fg),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        truncate_pad(&format_tokens(s.tokens), 6),
                        Style::default().fg(p.fg),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        truncate_pad(&format_cost(s.cost_usd), 6),
                        Style::default().fg(p.muted),
                    ),
                ]);
                ListItem::new(line)
            })
            .collect();
        f.render_widget(List::new(items), bot_split[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_variants() {
        assert_eq!(format_tokens(None), "—");
        assert_eq!(format_tokens(Some(-5)), "—");
        assert_eq!(format_tokens(Some(0)), "0");
        assert_eq!(format_tokens(Some(999)), "999");
        assert_eq!(format_tokens(Some(1500)), "1.5k");
        assert_eq!(format_tokens(Some(42_000)), "42.0k");
        assert_eq!(format_tokens(Some(1_500_000)), "1.5M");
    }

    #[test]
    fn format_cost_variants() {
        assert_eq!(format_cost(None), "—");
        assert_eq!(format_cost(Some(-0.10)), "—");
        assert_eq!(format_cost(Some(f64::NAN)), "—");
        assert_eq!(format_cost(Some(0.314)), "$0.31");
        assert_eq!(format_cost(Some(12.5)), "$12.50");
    }

    #[test]
    fn format_ctx_variants() {
        assert_eq!(format_ctx(None), "—");
        assert_eq!(format_ctx(Some(-1)), "—");
        assert_eq!(format_ctx(Some(0)), "0%");
        assert_eq!(format_ctx(Some(42)), "42%");
        assert_eq!(format_ctx(Some(100)), "100%");
        assert_eq!(format_ctx(Some(101)), "—");
    }

    // ---- spec 001 -----------------------------------------------------

    #[test]
    fn band_color_thresholds() {
        // Use a palette whose three band colors are distinct so the
        // boundary assertions are unambiguous. `midnight` (BUILTINS[1])
        // has concrete (non-Reset) success/warning/error.
        let p = &super::super::theme::Theme::by_name("midnight").palette;
        // 🟢 below 70
        assert_eq!(band_color(0.0, p), p.success);
        assert_eq!(band_color(69.0, p), p.success);
        // 🟡 70..=90 (boundaries inclusive)
        assert_eq!(band_color(70.0, p), p.warning);
        assert_eq!(band_color(89.0, p), p.warning);
        assert_eq!(band_color(90.0, p), p.warning);
        // 🔴 strictly above 90
        assert_eq!(band_color(91.0, p), p.error);
        assert_eq!(band_color(100.0, p), p.error);
    }

    #[test]
    fn band_glyph_thresholds() {
        assert_eq!(band_glyph(69.0), "🟢");
        assert_eq!(band_glyph(70.0), "🟡");
        assert_eq!(band_glyph(89.0), "🟡");
        assert_eq!(band_glyph(90.0), "🟡");
        assert_eq!(band_glyph(91.0), "🔴");
    }

    #[test]
    fn format_resets_in_grain() {
        // 2h in the future.
        assert_eq!(format_resets_in(7_200_000, 0), "2h");
        // 30m.
        assert_eq!(format_resets_in(1_800_000, 0), "30m");
        // Under a minute / past → "now".
        assert_eq!(format_resets_in(30_000, 0), "now");
        assert_eq!(format_resets_in(0, 1_000_000), "now");
        // 2d.
        assert_eq!(format_resets_in(2 * 86_400_000, 0), "2d");
    }
}

// ── end Goal overlay ──────────────────────────────────────────────────────────
