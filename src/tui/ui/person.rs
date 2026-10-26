use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui_image::Image;

use crate::tui::app::{App, PageLink, STAFF_PORTRAIT, THUMB_COVER};
use crate::tui::model::{Media, PersonPageData};
use crate::tui::ui::text::{dim, plain_text, truncate};

const LABEL_WIDTH: usize = 14;

enum Row {
    Label(String),
    Gap,
    Work { role: String, media: Box<Media> },
}

impl Row {
    fn height(&self) -> u16 {
        match self {
            Row::Work { .. } => THUMB_COVER.1,
            _ => 1,
        }
    }

    fn link_count(&self) -> usize {
        match self {
            Row::Work { .. } => 1,
            _ => 0,
        }
    }
}

/// AniList-style staff/character page: portrait and profile on the left,
/// filmography on the right.
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .areas(area);

    let (loading, data) = match &app.person_view {
        Some(view) => (view.loading, view.data.clone()),
        None => return,
    };

    let page_id = data.as_ref().map(|data| data.id).unwrap_or(0);
    let url = data
        .as_ref()
        .map(|data| data.kind.url_path())
        .unwrap_or("staff");
    draw_top_bar(f, url, page_id, top);

    let [left, divider_area, right] = Layout::horizontal([
        Constraint::Length(34),
        Constraint::Length(1),
        Constraint::Min(24),
    ])
    .areas(body);

    let [portrait_area, info_area] =
        Layout::vertical([Constraint::Length(STAFF_PORTRAIT.1), Constraint::Min(1)]).areas(left);

    draw_portrait(f, app, data.as_ref(), portrait_area);

    let fields = data.as_ref().map(profile_lines).unwrap_or_default();
    f.render_widget(Paragraph::new(fields).wrap(Wrap { trim: true }), info_area);
    draw_divider(f, divider_area);

    let rows = match &data {
        Some(data) => work_rows(data),
        None => vec![Row::Label(if loading {
            "loading…".to_string()
        } else {
            "—".to_string()
        })],
    };

    let total: u16 = rows.iter().map(Row::height).sum();
    let max = total.saturating_sub(right.height);

    let Some(view) = &mut app.person_view else {
        return;
    };
    view.scroll = view.scroll.min(max);
    let scroll = view.scroll;

    if view.reveal {
        if let Some(cursor) = view.cursor
            && let Some(&(top, height)) = link_offsets(&rows).get(cursor)
        {
            if top < scroll {
                view.scroll = top;
            } else if top + height > scroll + right.height {
                view.scroll = (top + height).saturating_sub(right.height);
            }
        }
        view.reveal = false;
    }

    let scroll = view.scroll;
    let cursor = view.cursor;
    let near_end = scroll + right.height + 4 >= total;
    if near_end {
        app.prefetch_person();
    }

    let cursor_visible = draw_rows(f, app, &rows, right, scroll, cursor);
    if let Some(view) = &mut app.person_view {
        view.cursor_visible = cursor_visible;
    }

    let left_hint = " n/p link · j/k or ↑↓ scroll · g/G top/bottom · Enter open · Esc/⌫ back";
    f.render_widget(
        Paragraph::new(Span::styled(left_hint, Style::new().fg(Color::DarkGray))),
        bottom,
    );
    if max > 0 {
        f.render_widget(
            Paragraph::new(dim(format!("{scroll}/{max} "))).alignment(Alignment::Right),
            bottom,
        );
    }
}

fn link_offsets(rows: &[Row]) -> Vec<(u16, u16)> {
    let mut y: u16 = 0;
    let mut offsets = Vec::new();
    for row in rows {
        let height = row.height();
        for _ in 0..row.link_count() {
            offsets.push((y, height));
        }
        y = y.saturating_add(height);
    }
    offsets
}

fn work_rows(data: &PersonPageData) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    for (index, section) in data.sections.iter().enumerate() {
        if index > 0 {
            rows.push(Row::Gap);
        }
        rows.push(Row::Label(format!(
            "{} ({})",
            section.title,
            section.works.len()
        )));
        if section.works.is_empty() {
            rows.push(Row::Label("  none".to_string()));
        }
        for work in &section.works {
            rows.push(Row::Work {
                role: work.role.clone(),
                media: Box::new(work.media.clone()),
            });
        }
    }

    if rows.is_empty() {
        rows.push(Row::Label("no works yet".to_string()));
    }

    rows
}

/// Returns whether the focused row was drawn.
fn draw_rows(
    f: &mut Frame,
    app: &mut App,
    rows: &[Row],
    area: Rect,
    scroll: u16,
    cursor: Option<usize>,
) -> bool {
    let mut cursor_drawn = false;
    let mut link_index = 0usize;
    let mut y = area.y as i32 - scroll as i32;

    for row in rows {
        let height = row.height();
        let top = y;
        y += height as i32;

        let first_link = link_index;
        link_index += row.link_count();

        if y <= area.y as i32 || top >= area.bottom() as i32 {
            continue;
        }

        let visible_top = top.max(area.y as i32) as u16;
        let visible_bottom = (top + height as i32).min(area.bottom() as i32) as u16;
        let rect = Rect::new(
            area.x,
            visible_top,
            area.width,
            visible_bottom.saturating_sub(visible_top),
        );
        if rect.height == 0 {
            continue;
        }

        match row {
            Row::Gap => {}
            Row::Label(text) => {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        truncate(text, rect.width as usize),
                        Style::new().fg(Color::DarkGray),
                    )),
                    rect,
                );
            }
            Row::Work { role, media } => {
                let focused = cursor == Some(first_link);
                if focused {
                    cursor_drawn = true;
                }
                draw_work(f, app, role, media, rect, height, focused);
                app.page_links.push((rect, PageLink::Media(media.clone())));
            }
        }
    }

    cursor_drawn
}

fn draw_work(
    f: &mut Frame,
    app: &mut App,
    role: &str,
    media: &Media,
    rect: Rect,
    full_height: u16,
    focused: bool,
) {
    let thumb_width = THUMB_COVER.0.min(rect.width);
    let thumb_height = full_height.min(rect.height);
    let thumb_area = Rect::new(rect.x, rect.y, thumb_width, thumb_height);

    match media.cover_url().and_then(|url| {
        app.cover(
            &format!("{}:{}:cover", media.kind.label(), media.id),
            url,
            THUMB_COVER.0,
            THUMB_COVER.1,
        )
    }) {
        Some(proto) => {
            let size = proto.size();
            f.render_widget(
                Image::new(&proto),
                Rect::new(
                    thumb_area.x,
                    thumb_area.y,
                    size.width.min(thumb_area.width),
                    size.height.min(thumb_area.height),
                ),
            );
        }
        None => {
            f.render_widget(
                Paragraph::new(dim("···")).alignment(Alignment::Center),
                Rect::new(thumb_area.x, thumb_area.y, thumb_area.width, 1),
            );
        }
    }

    let text_x = rect.x + THUMB_COVER.0 + 1;
    let text_width = rect.right().saturating_sub(text_x);
    if text_width < 6 {
        return;
    }

    let mut year = String::new();
    if let Some(value) = media.season_year {
        year = format!(" {value}");
    }
    let title_room =
        (text_width as usize).saturating_sub(role.chars().count().min(18) + year.len() + 2);

    let title_style = if focused {
        Style::new()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::White)
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(truncate(media.title(), title_room), title_style),
            Span::styled(year, Style::new().fg(Color::DarkGray)),
        ])),
        Rect::new(text_x, rect.y, text_width, 1),
    );

    if rect.height >= 2 {
        f.render_widget(
            Paragraph::new(truncate(role, text_width as usize)).style(Style::new().fg(Color::Cyan)),
            Rect::new(text_x, rect.y + 1, text_width, 1),
        );
    }
}

fn draw_portrait(f: &mut Frame, app: &mut App, data: Option<&PersonPageData>, area: Rect) {
    let Some(data) = data else {
        f.render_widget(Paragraph::new(dim("···")), area);
        return;
    };
    let Some(url) = data.image.as_deref() else {
        f.render_widget(Paragraph::new(dim("···")), area);
        return;
    };
    let key = format!("{}:{}:portrait", data.kind.url_path(), data.id);

    match app.cover(&key, url, STAFF_PORTRAIT.0, STAFF_PORTRAIT.1) {
        Some(proto) => {
            let size = proto.size();
            f.render_widget(
                Image::new(&proto),
                Rect::new(
                    area.x,
                    area.y,
                    size.width.min(area.width),
                    size.height.min(area.height),
                ),
            );
        }
        None => {
            f.render_widget(Paragraph::new(dim("···")), area);
        }
    }
}

fn profile_lines(data: &PersonPageData) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        truncate(&data.name, 32),
        Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(""));

    for (label, value) in &data.fields {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{label:<LABEL_WIDTH$}"),
                Style::new().fg(Color::DarkGray),
            ),
            Span::styled(truncate(value, 40), Style::new().fg(Color::Gray)),
        ]));
    }

    if let Some(description) = data.description.as_deref() {
        let text = plain_text(description);
        if !text.is_empty() {
            lines.push(Line::from(""));
            for line in text.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::new().fg(Color::Gray),
                )));
            }
        }
    }

    lines
}

fn draw_divider(f: &mut Frame, area: Rect) {
    let line = "│\n".repeat(area.height as usize);
    f.render_widget(
        Paragraph::new(Span::styled(line, Style::new().fg(Color::DarkGray))),
        area,
    );
}

fn draw_top_bar(f: &mut Frame, path: &str, id: i64, area: Rect) {
    let left = Line::from(vec![
        Span::styled(
            " ◂ Esc/⌫ ",
            Style::new().fg(Color::Black).bg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled("aniscrobble", Style::new().fg(Color::Cyan)),
        dim(format!("  anilist.co/{path}/{id}")),
    ]);
    f.render_widget(Paragraph::new(left), area);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc;

    use crate::tui::app::{App, PageLink, PersonView};
    use crate::tui::model::{Media, PersonKind, PersonPageData, PersonSection, PersonWork, Title};

    fn work(id: i64, title: &str, year: i64) -> Media {
        Media {
            id,
            title: Title {
                romaji: Some(title.into()),
                english: None,
            },
            season_year: Some(year),
            ..Default::default()
        }
    }

    fn staff_page() -> PersonPageData {
        PersonPageData {
            kind: PersonKind::Staff,
            id: 97349,
            name: "Hiroyuki Imaishi".into(),
            image: None,
            description: Some("<p>Known for <b>Trigger</b>.</p>".into()),
            fields: vec![
                ("Language".into(), "Japanese".into()),
                ("Years Active".into(), "1995 - 2020".into()),
                ("Favourites".into(), "12,345".into()),
            ],
            sections: vec![
                PersonSection {
                    title: "ANIME STAFF POSITIONS".into(),
                    works: vec![
                        PersonWork {
                            role: "Director".into(),
                            media: work(777, "Promare", 2019),
                        },
                        PersonWork {
                            role: "Storyboard".into(),
                            media: work(2, "Kill la Kill", 2013),
                        },
                    ],
                },
                PersonSection {
                    title: "VOICE ACTING ROLES".into(),
                    works: vec![PersonWork {
                        role: "Some Character".into(),
                        media: work(3, "Example Show", 2021),
                    }],
                },
            ],
            has_next: false,
        }
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| super::draw(frame, app, frame.area()))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[tokio::test]
    async fn person_page_renders_profile_and_sections() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);
        app.set_person_for_test(PersonView {
            data: Some(staff_page()),
            page: 1,
            cursor_visible: true,
            ..Default::default()
        });

        let text = render(&mut app, 120, 40);
        for needle in [
            "Hiroyuki Imaishi",
            "Japanese",
            "1995 - 2020",
            "12,345",
            "Known for Trigger.",
            "ANIME STAFF POSITIONS (2)",
            "Promare",
            "2019",
            "Kill la Kill",
            "VOICE ACTING ROLES (1)",
            "Some Character",
            "Example Show",
        ] {
            assert!(text.contains(needle), "person page is missing {needle:?}");
        }
    }

    #[tokio::test]
    async fn character_page_uses_the_character_url() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);
        app.set_person_for_test(PersonView {
            data: Some(PersonPageData {
                kind: PersonKind::Character,
                id: 555,
                name: "D".into(),
                fields: vec![("Gender".into(), "Male".into())],
                sections: vec![PersonSection {
                    title: "ANIME ROLES".into(),
                    works: vec![PersonWork {
                        role: "MAIN".into(),
                        media: work(1, "Cyberpunk", 2022),
                    }],
                }],
                ..Default::default()
            }),
            page: 1,
            cursor_visible: true,
            ..Default::default()
        });

        let text = render(&mut app, 120, 40);
        assert!(text.contains("anilist.co/character/555"));
        assert!(text.contains("ANIME ROLES (1)"));
        assert!(text.contains("Gender"));
    }

    #[tokio::test]
    async fn work_rows_are_clickable_links() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);
        app.set_items_for_test(vec![Media {
            id: 1,
            title: Title {
                romaji: Some("Origin".into()),
                english: None,
            },
            ..Default::default()
        }]);
        app.detail = true;
        app.set_person_for_test(PersonView {
            data: Some(staff_page()),
            page: 1,
            cursor_visible: true,
            ..Default::default()
        });

        render(&mut app, 120, 40);

        let links = app.page_links_for_test();
        let (rect, link) = links.first().expect("work row is a link");
        assert!(matches!(link, PageLink::Media(_)));

        app.on_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });

        assert!(!app.person_open(), "clicking a work leaves the person page");
        assert_eq!(app.page_media().map(|media| media.id), Some(777));
        assert_eq!(app.history_depth(), 1, "the origin media is remembered");
    }
}
