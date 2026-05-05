//! Pure draw functions for the TUI. No mutation, no IO.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use tui_term::widget::PseudoTerminal;

use super::app::{App, ConnState, Focus, Row, status_dot};

const FOCUS_BORDER: Color = Color::Cyan;
const IDLE_BORDER: Color = Color::DarkGray;

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let area = f.area();

    // Vertical: title (1) + body (flex) + status (1)
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_title(f, vertical[0], app);

    // Horizontal: tree (32) + right column (flex)
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(32), Constraint::Min(20)])
        .split(vertical[1]);

    draw_tree(f, body[0], app);

    // Right column: terminal pane (flex) + input (3 lines incl borders)
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(body[1]);

    draw_terminal(f, right[0], app);
    draw_input(f, right[1], app);

    draw_status(f, vertical[2], app);

    if app.help {
        draw_help_overlay(f, area);
    }
}

fn draw_title(f: &mut Frame<'_>, area: Rect, app: &App) {
    let title = match app.selected_session() {
        Some(s) => format!("agentum · {} · {}", s.name, s.workdir),
        None => "agentum · no session selected".to_string(),
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
            format!("{} error{}", app.error_count, if app.error_count == 1 { "" } else { "s" }),
            Style::default().fg(Color::Red),
        )
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
        Span::raw(extra),
        Span::raw("   "),
        Span::styled("? help", Style::default().fg(Color::DarkGray)),
    ]);
    let para = Paragraph::new(bar).style(Style::default().bg(Color::Rgb(20, 25, 35)));
    f.render_widget(para, area);
}

fn draw_help_overlay(f: &mut Frame<'_>, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "agentum tui — keys",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  j / k / ↑ / ↓   move selection"),
        Line::from("  h / l           collapse / expand workdir group"),
        Line::from("  Enter           select session (start streaming)"),
        Line::from("  i               focus the input bar"),
        Line::from("  Enter (input)   send + append Enter to the pane"),
        Line::from("  Esc             leave input"),
        Line::from("  Tab             cycle focus tree → term → input"),
        Line::from("  r               refresh sessions"),
        Line::from("  ?               toggle help"),
        Line::from("  q / Ctrl-C      quit"),
    ];
    let w = 56u16;
    let h = lines.len() as u16 + 2;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let r = Rect {
        x,
        y,
        width: w.min(area.width),
        height: h.min(area.height),
    };
    let block = Block::default()
        .title(" help ")
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
