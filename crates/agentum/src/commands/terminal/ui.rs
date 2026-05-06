//! Pure draw functions for the terminal dashboard. No mutation, no IO.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use tui_term::widget::PseudoTerminal;

use super::app::{
    App, ConnState, DirPickerState, Focus, NewSessionField, NewSessionForm, Overlay,
    PendingAction, Row, palette_catalog, status_dot,
};
use super::extensions::{self, Extension, LAZYGIT};
use super::theme::Palette;

const TREE_WIDTH: u16 = 32;

#[derive(Clone, Copy)]
pub struct Areas {
    pub title: Rect,
    pub tree: Rect,
    pub terminal: Rect,
    pub lazygit: Option<Rect>,
    pub status: Rect,
}

pub fn compute_layout(area: Rect, lazygit_open: bool, fullscreen: bool) -> Areas {
    // Fullscreen: drop the title row, tree column, and status row so the
    // active panes consume every available cell. The empty Rects keep the
    // draw_* helpers no-op (they short-circuit on `area.width == 0`).
    if fullscreen {
        let (terminal_rect, lazygit_rect) = split_main(area, lazygit_open);
        let empty = Rect {
            x: area.x,
            y: area.y,
            width: 0,
            height: 0,
        };
        return Areas {
            title: empty,
            tree: empty,
            terminal: terminal_rect,
            lazygit: lazygit_rect,
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

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(TREE_WIDTH), Constraint::Min(20)])
        .split(v[1]);

    // Right column is now 100% terminal/lazygit — no bottom input bar.
    let (terminal_rect, lazygit_rect) = split_main(body[1], lazygit_open);

    Areas {
        title: v[0],
        tree: body[0],
        terminal: terminal_rect,
        lazygit: lazygit_rect,
        status: v[2],
    }
}

fn split_main(area: Rect, lazygit_open: bool) -> (Rect, Option<Rect>) {
    if !lazygit_open {
        return (area, None);
    }
    if area.width >= 100 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
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

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let areas = compute_layout(f.area(), app.lazygit_open(), app.fullscreen);
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
    if let Some(lg_area) = areas.lazygit {
        draw_lazygit(f, lg_area, app, p);
    }
    if areas.status.height > 0 {
        draw_status(f, areas.status, app, p);
    }

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => draw_help_overlay(f, f.area(), app.lazygit_open(), p),
        Overlay::LazygitCheats => draw_cheatsheet_overlay(f, f.area(), &LAZYGIT, p),
        Overlay::LazygitInstall => draw_install_overlay(f, f.area(), &LAZYGIT, p),
        Overlay::Palette => draw_palette_overlay(f, f.area(), app, p),
        Overlay::NewSession(form) => draw_new_session_overlay(f, f.area(), form, p),
        Overlay::Confirm(action) => draw_confirm_overlay(f, f.area(), action, p),
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
    // Top bar is intentionally minimal: app name + active session.
    // Theme chip + ops info live in the status bar so we don't render the
    // same data twice (workdir, theme, "Ctrl-P palette" used to repeat).
    let title = match app.selected_session() {
        Some(s) => format!(" agentum · {} ", s.name),
        None => " agentum · no session selected ".to_string(),
    };
    let line = Line::from(vec![Span::styled(
        title,
        Style::default()
            .fg(p.fg_strong)
            .bg(p.body_bg)
            .add_modifier(Modifier::BOLD),
    )]);
    let para = Paragraph::new(line).style(Style::default().bg(p.body_bg));
    f.render_widget(para, area);
}

fn draw_tree(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let focused = app.focus == Focus::Tree;
    let block = panel_block(" 1 sessions ", focused, p);

    let mut items: Vec<ListItem> = Vec::new();
    let cursor = app.tree.cursor;
    for (i, row) in app.tree.rows().iter().enumerate() {
        let is_cursor = i == cursor;
        items.push(render_tree_row(app, *row, is_cursor, focused, p));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no sessions — `agentum new …`)",
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
            let label = collapse_home(&g.workdir);
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
                    let (dot, color) = status_dot(s.status);
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
    let title = if focused {
        " 2 terminal · Ctrl-E release · type freely "
    } else {
        " 2 terminal · 2 / Ctrl-Shift-] focus "
    };
    let block = panel_block(title, focused, p);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.selected.is_none() {
        let hint = Paragraph::new("Select a session on the left and press Enter.")
            .style(Style::default().fg(p.muted).bg(p.panel_bg))
            .wrap(Wrap { trim: true });
        f.render_widget(hint, inner);
        return;
    }

    let pseudo = PseudoTerminal::new(app.term.screen());
    f.render_widget(pseudo, inner);
    fill_default_bg(f, inner, p.panel_bg);
}

fn draw_lazygit(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let focused = app.focus == Focus::Lazygit;
    let title = if focused {
        " 3 lazygit · Ctrl-E release · G cheats "
    } else {
        " 3 lazygit · 3 / Ctrl-Shift-] focus · g close "
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

fn draw_status(f: &mut Frame<'_>, area: Rect, app: &App, p: &Palette) {
    let workdir = app
        .selected_session()
        .map(|s| collapse_home(&s.workdir))
        .unwrap_or_else(|| "—".to_string());

    let tool_label = app
        .selected_session()
        .map(|s| match s.model.as_deref() {
            Some(m) => format!("{} · {}", s.tool, m),
            None => s.tool.clone(),
        })
        .unwrap_or_else(|| "—".to_string());

    let (conn_label, conn_color) = match app.conn {
        ConnState::Connected => ("● connected", p.success),
        ConnState::Connecting => ("◌ connecting", p.warning),
        ConnState::Disconnected => ("✗ disconnected", p.error),
    };

    let err_text = if app.error_count == 0 {
        Span::styled(" 0 errors ", Style::default().fg(p.muted).bg(p.chrome_bg))
    } else {
        Span::styled(
            format!(
                " {} error{} ",
                app.error_count,
                if app.error_count == 1 { "" } else { "s" }
            ),
            Style::default()
                .fg(p.error)
                .bg(p.chrome_bg)
                .add_modifier(Modifier::BOLD),
        )
    };

    let lg_chip = if app.lazygit_open() {
        Span::styled(
            " lazygit ",
            Style::default()
                .bg(p.chip_bg)
                .fg(p.chip_fg)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" g lazygit ", Style::default().fg(p.muted).bg(p.chrome_bg))
    };

    // Bottom-left notification: show the most recent notification.
    let notif = if let Some(n) = app.notifications.last() {
        Span::styled(
            format!(" ● {n} "),
            Style::default()
                .fg(p.accent_alt)
                .bg(p.chrome_bg)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("", Style::default().fg(p.muted).bg(p.chrome_bg))
    };

    let extra = match &app.status_msg {
        Some(m) => format!(" · {m} "),
        None => String::new(),
    };

    let bar = Line::from(vec![
        notif,
        Span::styled(
            format!(" {workdir} "),
            Style::default().fg(p.fg).bg(p.chrome_bg),
        ),
        Span::styled(
            format!("{tool_label} "),
            Style::default().fg(p.muted).bg(p.chrome_bg),
        ),
        Span::styled(
            format!(" {conn_label} "),
            Style::default().fg(conn_color).bg(p.chrome_bg),
        ),
        err_text,
        lg_chip,
        Span::styled(
            format!(" {} ", app.theme.name),
            Style::default().fg(p.chip_fg).bg(p.chip_bg),
        ),
        Span::styled(extra, Style::default().fg(p.muted).bg(p.chrome_bg)),
        Span::styled(
            " Ctrl-P palette ",
            Style::default().fg(p.accent).bg(p.chrome_bg),
        ),
        Span::styled(" ? help ", Style::default().fg(p.muted).bg(p.chrome_bg)),
    ]);
    let para = Paragraph::new(bar).style(Style::default().bg(p.chrome_bg).fg(p.fg));
    f.render_widget(para, area);
}

fn draw_help_overlay(f: &mut Frame<'_>, area: Rect, lazygit_open: bool, p: &Palette) {
    let mut lines = vec![
        head("agentum terminal — keys", p),
        Line::from(""),
        head("  Universal (work even inside the terminal pane)", p),
        body("  Ctrl-P / Ctrl-Shift-P  command palette", p),
        body("  Ctrl-E            release pane focus → tree", p),
        body("  Ctrl-Shift-] / F5  next panel", p),
        body("  Ctrl-Shift-[ / F6  previous panel", p),
        body("  Ctrl-1 … Ctrl-9   jump to Nth project group in the tree", p),
        body("  Ctrl-Q            quit", p),
        body("  Ctrl-C            interrupt focused pane (else quit)", p),
        Line::from(""),
        head("  Tree", p),
        body("  1 / 2 / 3         focus tree / terminal / lazygit", p),
        body("  Tab / ]           next panel", p),
        body("  Shift-Tab / [     previous panel", p),
        body("  j / k / ↑ / ↓     move selection", p),
        body("  h / l / ← / →     collapse / expand group", p),
        body(
            "  Enter             select session and focus the terminal",
            p,
        ),
        body("  r                 refresh sessions", p),
        body("  t                 spawn plain bash terminal", p),
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
            "  K                 kill the selected session (immediate)",
            p,
        ),
        body("  D                 delete the selected session", p),
        Line::from(""),
        head("  Extensions & appearance", p),
        body("  g                 toggle lazygit side pane", p),
        body("  G                 lazygit cheat sheet", p),
        body("  T                 cycle theme", p),
        body(
            "  Shift-F           toggle fullscreen (hide tree + chrome)",
            p,
        ),
        body("  Esc               exit fullscreen", p),
        Line::from(""),
        body("  ?                 toggle this help", p),
        body("  q                 quit (when tree is focused)", p),
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

    // Hints line — Fresh-style prefix legend.
    let hints = Line::from(vec![
        Span::styled(" › ", Style::default().fg(p.subtle)),
        Span::styled("type", Style::default().fg(p.muted)),
        Span::styled("  >", Style::default().fg(p.accent)),
        Span::styled(" commands", Style::default().fg(p.muted)),
        Span::styled("  #", Style::default().fg(p.accent)),
        Span::styled(" sessions", Style::default().fg(p.muted)),
        Span::styled("  @", Style::default().fg(p.accent)),
        Span::styled(" themes", Style::default().fg(p.muted)),
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

fn draw_new_session_overlay(f: &mut Frame<'_>, area: Rect, form: &NewSessionForm, p: &Palette) {
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
    push_form_field_with_hint(
        &mut lines,
        "Tool",
        &form.tool,
        form.field == NewSessionField::Tool,
        "claude",
        Some("Tab cycles claude → codex → opencode → aider"),
        p,
    );
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
        Some("Enter opens the folder picker"),
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
        // User toggled it on but the current tool doesn't accept the flag.
        "YOLO mode (--dangerously-skip-permissions, ignored for this tool)"
    } else {
        "YOLO mode (--dangerously-skip-permissions)"
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
