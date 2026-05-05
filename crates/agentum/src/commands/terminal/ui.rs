//! Pure draw functions for the terminal dashboard. No mutation, no IO.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use tui_term::widget::PseudoTerminal;

use super::app::{App, ConnState, Focus, Overlay, Row, status_dot};
use super::extensions::{self, Extension, LAZYGIT};

const FOCUS_BORDER: Color = Color::Cyan;
const IDLE_BORDER: Color = Color::DarkGray;
const TREE_WIDTH: u16 = 32;

/// Computed pane rectangles. Mirrored by `app::run_loop` so the vt100
/// parsers can be sized to match.
#[derive(Clone, Copy)]
pub struct Areas {
    pub title: Rect,
    pub tree: Rect,
    pub terminal: Rect,
    pub lazygit: Option<Rect>,
    pub input: Rect,
    pub status: Rect,
}

/// Single source of truth for the layout. Called from both `run_loop`
/// (to size PTY parsers) and `draw` (to render).
pub fn compute_layout(area: Rect, lazygit_open: bool) -> Areas {
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

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(body[1]);

    let (terminal_rect, lazygit_rect) = if lazygit_open {
        // Pick orientation by available width: side-by-side when there's
        // enough room, stacked otherwise.
        if right[0].width >= 100 {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(right[0]);
            (cols[0], Some(cols[1]))
        } else {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(right[0]);
            (rows[0], Some(rows[1]))
        }
    } else {
        (right[0], None)
    };

    Areas {
        title: v[0],
        tree: body[0],
        terminal: terminal_rect,
        lazygit: lazygit_rect,
        input: right[1],
        status: v[2],
    }
}

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let areas = compute_layout(f.area(), app.lazygit_open());

    draw_title(f, areas.title, app);
    draw_tree(f, areas.tree, app);
    draw_terminal(f, areas.terminal, app);
    if let Some(lg_area) = areas.lazygit {
        draw_lazygit(f, lg_area, app);
    }
    draw_input(f, areas.input, app);
    draw_status(f, areas.status, app);

    match app.overlay {
        Overlay::None => {}
        Overlay::Help => draw_help_overlay(f, f.area(), app.lazygit_open()),
        Overlay::LazygitCheats => draw_cheatsheet_overlay(f, f.area(), &LAZYGIT),
        Overlay::LazygitInstall => draw_install_overlay(f, f.area(), &LAZYGIT),
    }
}

fn draw_title(f: &mut Frame<'_>, area: Rect, app: &App) {
    let title = match app.selected_session() {
        Some(s) => format!("agentum terminal · {} · {}", s.name, s.workdir),
        None => "agentum terminal · no session selected".to_string(),
    };
    let para = Paragraph::new(title).style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(para, area);
}

fn draw_tree(f: &mut Frame<'_>, area: Rect, app: &App) {
    let focused = app.focus == Focus::Tree;
    let block = Block::default()
        .title(" sessions ")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    let mut items: Vec<ListItem> = Vec::new();
    let cursor = app.tree.cursor;
    for (i, row) in app.tree.rows().iter().enumerate() {
        let is_cursor = i == cursor;
        items.push(render_tree_row(app, *row, is_cursor));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no sessions — `agentum new …`)",
            Style::default().fg(Color::DarkGray),
        ))));
    }

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn render_tree_row(app: &App, row: Row, is_cursor: bool) -> ListItem<'static> {
    let cursor_bg = if is_cursor {
        Style::default().bg(Color::Rgb(40, 60, 70))
    } else {
        Style::default()
    };
    match row {
        Row::Group(gi) => {
            let g = &app.tree.groups[gi];
            let arrow = if g.expanded { "▾" } else { "▸" };
            let label = collapse_home(&g.workdir);
            ListItem::new(Line::from(vec![
                Span::raw(format!("{arrow} ")),
                Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
            ]))
            .style(cursor_bg)
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
                None => ("?".into(), "?", Color::Red, "".into()),
            };
            let mut spans = vec![
                Span::raw("   "),
                Span::styled(format!("{:<14}", truncate(&name, 14)), Style::default()),
                Span::raw(" "),
                Span::styled(dot, Style::default().fg(dot_color)),
                Span::raw(" "),
                Span::styled(tool_label, Style::default().fg(Color::DarkGray)),
            ];
            if is_cursor {
                spans[1].style = spans[1].style.add_modifier(Modifier::BOLD);
            }
            ListItem::new(Line::from(spans)).style(cursor_bg)
        }
    }
}

fn draw_terminal(f: &mut Frame<'_>, area: Rect, app: &App) {
    let focused = app.focus == Focus::Term;
    let block = Block::default()
        .title(" terminal ")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.selected.is_none() {
        let hint = Paragraph::new("Select a session on the left and press Enter.")
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true });
        f.render_widget(hint, inner);
        return;
    }

    let pseudo = PseudoTerminal::new(app.term.screen());
    f.render_widget(pseudo, inner);
}

fn draw_lazygit(f: &mut Frame<'_>, area: Rect, app: &App) {
    let focused = app.focus == Focus::Lazygit;
    let title = if focused {
        " lazygit · Ctrl-G to release · G for cheat sheet "
    } else {
        " lazygit · Tab to focus · g to close "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(lg) = app.lazygit.as_ref() {
        let pseudo = PseudoTerminal::new(lg.screen());
        f.render_widget(pseudo, inner);
    } else {
        let hint =
            Paragraph::new("lazygit not running").style(Style::default().fg(Color::DarkGray));
        f.render_widget(hint, inner);
    }
}

fn draw_input(f: &mut Frame<'_>, area: Rect, app: &App) {
    let focused = app.focus == Focus::Input;
    let placeholder = if app.input.is_empty() && !focused {
        "Press i to type a message or @path/to/file"
    } else {
        ""
    };
    let prompt = if focused { "> " } else { "  " };
    let line = if app.input.is_empty() && !focused {
        Line::from(vec![
            Span::raw(prompt),
            Span::styled(placeholder, Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![Span::raw(prompt), Span::raw(app.input.clone())])
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(focused));
    let para = Paragraph::new(line).block(block);
    f.render_widget(para, area);

    if focused {
        // Cursor inside the bordered area, after the prompt.
        let x = area.x + 1 + 2 + app.input.chars().count() as u16;
        let y = area.y + 1;
        f.set_cursor_position((x, y));
    }
}

fn draw_status(f: &mut Frame<'_>, area: Rect, app: &App) {
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
        ConnState::Connected => ("● connected", Color::Green),
        ConnState::Connecting => ("◌ connecting", Color::Yellow),
        ConnState::Disconnected => ("✗ disconnected", Color::Red),
    };

    let err_text = if app.error_count == 0 {
        Span::styled("0 errors", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(
            format!(
                "{} error{}",
                app.error_count,
                if app.error_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::Red),
        )
    };

    let lg_chip = if app.lazygit_open() {
        Span::styled(
            " lazygit ",
            Style::default()
                .bg(Color::Rgb(60, 30, 70))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("g lazygit", Style::default().fg(Color::DarkGray))
    };

    let extra = match &app.status_msg {
        Some(m) => format!(" · {m}"),
        None => String::new(),
    };

    let bar = Line::from(vec![
        Span::raw(" "),
        Span::raw(workdir),
        Span::raw("  "),
        Span::styled(tool_label, Style::default().fg(Color::DarkGray)),
        Span::raw("    "),
        Span::styled(conn_label, Style::default().fg(conn_color)),
        Span::raw("   "),
        err_text,
        Span::raw("   "),
        lg_chip,
        Span::raw(extra),
        Span::raw("   "),
        Span::styled("? help", Style::default().fg(Color::DarkGray)),
    ]);
    let para = Paragraph::new(bar).style(Style::default().bg(Color::Rgb(20, 25, 35)));
    f.render_widget(para, area);
}

fn draw_help_overlay(f: &mut Frame<'_>, area: Rect, lazygit_open: bool) {
    let mut lines = vec![
        Line::from(Span::styled(
            "agentum terminal — keys",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  j / k / ↑ / ↓   move selection"),
        Line::from("  h / l           collapse / expand workdir group"),
        Line::from("  Enter           select session (start streaming)"),
        Line::from("  i               focus the input bar"),
        Line::from("  Enter (input)   send + append Enter to the pane"),
        Line::from("  Esc             leave input"),
        Line::from("  Tab             cycle focus tree → term → (lazygit) → input"),
        Line::from("  r               refresh sessions"),
        Line::from(""),
        Line::from(Span::styled(
            "  Extensions",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  g               toggle lazygit side pane"),
        Line::from("  G               lazygit cheat sheet"),
        Line::from("  Ctrl-G          release lazygit focus → tree"),
        Line::from(""),
        Line::from("  ?               toggle this help"),
        Line::from("  q / Ctrl-C      quit"),
    ];
    if lazygit_open {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  (lazygit is open — keys forward to it when focused)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    overlay_box(f, area, " help ", lines, 64);
}

fn draw_cheatsheet_overlay(f: &mut Frame<'_>, area: Rect, ext: &Extension) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} — cheat sheet", ext.display_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (label, keys) in ext.cheatsheet {
        lines.push(Line::from(format!("  {:<22} {}", label, keys)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", ext.homepage),
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from("  Esc / Enter to dismiss"));
    overlay_box(f, area, &format!(" {} ", ext.display_name), lines, 64);
}

fn draw_install_overlay(f: &mut Frame<'_>, area: Rect, ext: &Extension) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} not found on PATH", ext.display_name),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(ext.blurb, Style::default().fg(Color::Gray))),
        Line::from(""),
        Line::from(Span::styled(
            "Install with one of:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    for (name, cmd) in extensions::install_hints(ext) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<22} ", name), Style::default().fg(Color::Cyan)),
            Span::raw(cmd),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", ext.homepage),
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from("  Esc / Enter to dismiss"));
    overlay_box(
        f,
        area,
        &format!(" install {} ", ext.display_name),
        lines,
        72,
    );
}

fn overlay_box(f: &mut Frame<'_>, area: Rect, title: &str, lines: Vec<Line<'_>>, width: u16) {
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
        .title(title.to_string())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let para = Paragraph::new(lines).block(block);
    f.render_widget(Clear, r);
    f.render_widget(para, r);
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(FOCUS_BORDER)
    } else {
        Style::default().fg(IDLE_BORDER)
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
