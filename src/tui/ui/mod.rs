use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::App;

pub mod list;
pub mod page;
pub mod person;
pub mod text;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(f: &mut Frame, app: &mut App) {
    if app.person_open() {
        person::draw(f, app, f.area());
        return;
    }
    if app.detail {
        page::draw(f, app, f.area());
        return;
    }

    let [header, search, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(f, app, header);
    draw_search(f, app, search);

    list::draw(f, app, body);
    draw_footer(f, app, footer);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let left = Line::from(vec![
        Span::styled(
            " aniscrobble ",
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(app.source.label(), Style::new().fg(Color::Gray)),
    ]);
    f.render_widget(Paragraph::new(left), area);

    let busy = app.list_loading || !app.inflight().eq(&0);
    let state = if busy {
        let frame = SPINNER[app.spinner as usize % SPINNER.len()];
        format!("{frame} {} req", app.inflight())
    } else {
        format!("{} req", app.inflight())
    };
    let right = format!(
        "{}/{}  {}  {} ",
        app.selected.saturating_add(1),
        app.items.len(),
        state,
        app.protocol
    );
    f.render_widget(
        Paragraph::new(Span::styled(right, Style::new().fg(Color::DarkGray)))
            .alignment(Alignment::Right),
        area,
    );
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let line = if app.search_focus {
        Line::from(vec![
            Span::styled("/ ", Style::new().fg(Color::Yellow)),
            Span::styled(app.search_input.clone(), Style::new().fg(Color::Yellow)),
            Span::styled("▏", Style::new().fg(Color::Yellow)),
            Span::styled(
                format!(
                    "  {} results · Enter to search · Esc to cancel",
                    app.source.label()
                ),
                Style::new().fg(Color::DarkGray),
            ),
        ])
    } else if app.search_input.trim().is_empty() {
        Line::from(Span::styled(
            "/ press / to search",
            Style::new().fg(Color::DarkGray),
        ))
    } else {
        Line::from(vec![
            Span::styled("/ ", Style::new().fg(Color::DarkGray)),
            Span::styled(app.search_input.clone(), Style::new().fg(Color::Gray)),
        ])
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.error {
        Some(err) => Line::from(vec![
            Span::styled(format!(" {err}"), Style::new().fg(Color::Red)),
            Span::styled("   r retry", Style::new().fg(Color::DarkGray)),
        ]),
        None => {
            let mut hint = String::from(
                " j/k move  Enter open  / search  Tab source  m/M my lists  r reload  q quit",
            );
            if app.has_browse_origin() {
                hint.push_str("  ⌫ back to page");
            }
            Line::from(Span::styled(hint, Style::new().fg(Color::DarkGray)))
        }
    };
    f.render_widget(Paragraph::new(line), area);
}
