use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui_image::Image;

use crate::tui::app::{App, LIST_COVER, Source};
use crate::tui::model::{Media, MediaKind};
use crate::tui::ui::text::{plain_text, truncate};

const GAP: u16 = 1;

struct Row {
    idx: usize,
    id: i64,
    title: String,
    meta: String,
    plot: String,
    url: Option<String>,
    new: bool,
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width < LIST_COVER.0 + 8 || area.height < LIST_COVER.1 {
        return;
    }

    app.list_targets.clear();

    if app.items.is_empty() {
        draw_empty(f, app, area);
        return;
    }

    let row_h = LIST_COVER.1 + GAP;
    let visible = (area.height / row_h).max(1) as usize;

    let max_offset = app.items.len().saturating_sub(visible);
    let offset = app.selected.saturating_sub(visible / 2).min(max_offset);
    let last = app.items.len().min(offset + visible);

    let rows: Vec<Row> = (offset..last)
        .map(|idx| {
            let media = &app.items[idx];
            let mut meta: Vec<String> = Vec::new();
            let mut new = false;

            if let Source::Mine = &app.source {
                let kind = media.kind;
                let progress = app.my_progress(media.id, kind).unwrap_or(0);
                meta.push(mine_meta(media, progress, kind));
                new = media
                    .available(kind)
                    .is_some_and(|available| available > progress);
            } else {
                if let Some(year) = media.season_year {
                    meta.push(year.to_string());
                }
                if let Some(format) = &media.format {
                    meta.push(format.clone());
                }
                if let Some(episodes) = media.episodes {
                    meta.push(format!("{episodes} ep"));
                }
                if let Some(score) = media.average_score {
                    meta.push(format!("★{score}"));
                }
                meta.extend(media.genres.iter().cloned());
            }

            Row {
                idx,
                id: media.id,
                title: media.title().to_string(),
                meta: meta.join(" · "),
                plot: media
                    .description
                    .as_deref()
                    .map(plain_text)
                    .unwrap_or_else(|| "no synopsis".to_string()),
                url: media.cover_url().map(str::to_string),
                new,
            }
        })
        .collect();

    let text_width = area.width - LIST_COVER.0 - 1;
    let headers = app.list_headers().to_vec();
    let mut y = area.y;

    for row in &rows {
        if let Some((_, label)) = headers.iter().find(|(index, _)| *index == row.idx) {
            if y + 1 > area.bottom() {
                break;
            }
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!(" {label}"),
                    Style::new()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )),
                Rect::new(area.x, y, area.width, 1),
            );
            y += 1;
        }
        if y + LIST_COVER.1 > area.bottom() {
            break;
        }

        let image_area = Rect::new(area.x, y, LIST_COVER.0, LIST_COVER.1);
        let kind = app
            .items
            .get(row.idx)
            .map_or(MediaKind::Anime, |media| media.kind);
        let proto = row.url.as_deref().and_then(|url| {
            app.cover(
                &format!("{}:{}:cover", kind.label(), row.id),
                url,
                LIST_COVER.0,
                LIST_COVER.1,
            )
        });

        match proto {
            Some(proto) => {
                let size = proto.size();
                f.render_widget(
                    Image::new(&proto),
                    centered(image_area, size.width, size.height),
                );
            }
            None => {
                f.render_widget(
                    Paragraph::new(Span::styled("···", Style::new().fg(Color::DarkGray)))
                        .alignment(Alignment::Center),
                    centered(image_area, 3, 1),
                );
            }
        }

        let selected = row.idx == app.selected;
        // The whole row block is clickable: image, title and synopsis lines.
        app.list_targets.push((
            Rect::new(area.x, y, area.width, row_h.min(area.bottom() - y)),
            row.id,
        ));
        let text_x = area.x + LIST_COVER.0 + 1;

        let title_style = if selected {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::White)
        };
        let marker = if selected { "▸ " } else { "" };
        let title = format!(
            "{marker}{}",
            truncate(&row.title, text_width.saturating_sub(2) as usize)
        );
        f.render_widget(
            Paragraph::new(Span::styled(title, title_style)),
            Rect::new(text_x, y, text_width, 1),
        );

        f.render_widget(
            Paragraph::new(Span::styled(
                truncate(&row.meta, text_width as usize),
                Style::new().fg(if row.new { Color::Yellow } else { Color::Cyan }),
            )),
            Rect::new(text_x, y + 1, text_width, 1),
        );

        let plot_height = LIST_COVER.1.saturating_sub(2);
        if plot_height > 0 {
            f.render_widget(
                Paragraph::new(row.plot.clone())
                    .wrap(ratatui::widgets::Wrap { trim: true })
                    .style(Style::new().fg(Color::Gray)),
                Rect::new(text_x, y + 2, text_width, plot_height),
            );
        }
        y += row_h;
    }
}

fn mine_meta(media: &Media, progress: i64, kind: MediaKind) -> String {
    let mut parts = Vec::new();
    if let Some(format) = &media.format {
        parts.push(format.clone());
    }
    parts.push(match media.total(kind) {
        Some(total) => format!("{progress} / {total}"),
        None => progress.to_string(),
    });
    if let Some(available) = media
        .available(kind)
        .filter(|available| *available > progress)
    {
        parts.push(format!("+{} new", available - progress));
    }
    parts.join(" · ")
}

fn draw_empty(f: &mut Frame, app: &App, area: Rect) {
    let message = match (&app.error, app.list_loading) {
        (Some(err), _) => format!("{err}\n\npress r to retry"),
        (None, true) => "loading…".to_string(),
        (None, false) => "no results".to_string(),
    };
    let style = if app.error.is_some() {
        Style::new().fg(Color::Red)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(style),
        centered(area, area.width.min(60), 3),
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc;

    use super::mine_meta;
    use crate::tui::app::App;
    use crate::tui::model::{AiringEpisode, Media, MediaKind, Title};

    fn sample(title: &str, id: i64) -> Media {
        Media {
            id,
            title: Title {
                romaji: Some(title.into()),
                english: None,
            },
            ..Default::default()
        }
    }

    #[test]
    fn my_list_meta_shows_progress_and_new_episodes() {
        let mut media = sample("Show", 1);
        media.format = Some("TV".into());
        media.episodes = Some(12);
        media.status = Some("RELEASING".into());
        media.next_airing_episode = Some(AiringEpisode { episode: 6 });

        assert_eq!(
            mine_meta(&media, 4, MediaKind::Anime),
            "TV · 4 / 12 · +1 new"
        );
        assert_eq!(mine_meta(&media, 5, MediaKind::Anime), "TV · 5 / 12");

        media.next_airing_episode = None;
        media.status = Some("FINISHED".into());
        assert_eq!(
            mine_meta(&media, 11, MediaKind::Anime),
            "TV · 11 / 12 · +1 new"
        );

        media.kind = MediaKind::Manga;
        media.chapters = Some(30);
        assert_eq!(
            mine_meta(&media, 12, MediaKind::Manga),
            "TV · 12 / 30 · +18 new"
        );
    }

    #[tokio::test]
    async fn clicking_a_list_row_opens_that_media() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);
        app.set_items_for_test(vec![sample("First", 1), sample("Second", 2)]);

        let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("terminal");
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");

        let targets = app.list_targets_for_test();
        assert_eq!(targets.len(), 2, "one target per visible row");
        let (rect, id) = targets[1];
        assert_eq!(id, 2);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 4,
            row: rect.y + 1,
            modifiers: KeyModifiers::NONE,
        });

        assert!(app.detail);
        assert_eq!(app.selected, 1, "clicked row becomes the selection");
        assert_eq!(app.page_media().map(|media| media.id), Some(2));
        assert_eq!(app.history_depth(), 0, "opening from the list is fresh");
    }
}
