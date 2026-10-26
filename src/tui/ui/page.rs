use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_image::Image;

use crate::tui::app::{
    App, CREDIT_COVER, CreditList, PAGE_COVER, PageLink, PagePane, PageTab, THUMB_COVER,
};
use crate::tui::model::{Media, MediaKind, MediaListEntry};
use crate::tui::ui::text::{dim, plain_text, truncate};

const LABEL_WIDTH: usize = 16;
const SIDEBAR_WIDTH: u16 = 34;

enum Item {
    Cover,
    Label(String),
    Text(String),
    Strong(String),
    Warn(String),
    Row {
        label: &'static str,
        value: String,
        color: Color,
    },
    /// One line of clickable chips (tags or genres).
    Chips(Vec<Chip>),
    Rule,
    Gap,
    /// One clickable media row: thumbnail on the left, title and facts right.
    Entry {
        prefix: String,
        media: Box<Media>,
    },
    /// Character or staff row: portrait, name, role. Staff rows are links to
    /// their own page; characters are informational for now.
    Person {
        name: String,
        role: String,
        image: Option<String>,
        link: Option<PageLink>,
    },
}

struct Chip {
    text: String,
    link: PageLink,
    color: Color,
}

impl Item {
    fn link_count(&self) -> usize {
        match self {
            Item::Chips(chips) => chips.len(),
            Item::Entry { .. } => 1,
            Item::Person { link: Some(_), .. } => 1,
            _ => 0,
        }
    }

    fn height(&self) -> u16 {
        match self {
            Item::Cover => PAGE_COVER.1,
            Item::Entry { .. } => THUMB_COVER.1,
            Item::Person { .. } => CREDIT_COVER.1,
            _ => 1,
        }
    }
}

fn total_height(items: &[Item]) -> u16 {
    items.iter().map(Item::height).sum()
}

/// Full-screen media page split in two independent columns: the sidebar (cover,
/// fields, tags) and the content (title, genres, synopsis, relations,
/// recommendations). `Tab` switches which one the keys scroll.
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(item) = app.page_media() else {
        return;
    };

    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_top_bar(f, app, &item.id, top);

    let [sidebar_area, divider_area, right] = Layout::horizontal([
        Constraint::Length(SIDEBAR_WIDTH),
        Constraint::Length(1),
        Constraint::Min(24),
    ])
    .areas(body);

    // Tabs belong to the content column only: the sidebar keeps its own layout.
    let [tabs_area, content_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(right);
    app.tab_targets.clear();
    draw_tabs(f, app, tabs_area);

    let sidebar = sidebar_items(app, &item, sidebar_area.width);
    let content = match app.tab {
        PageTab::Overview => content_items(app, &item, content_area.width),
        PageTab::Characters => credit_items(
            &app.characters_tab,
            "CHARACTERS",
            content_area.width,
            Some(crate::tui::model::PersonKind::Character),
        ),
        PageTab::Staff => credit_items(
            &app.staff_tab,
            "STAFF",
            content_area.width,
            Some(crate::tui::model::PersonKind::Staff),
        ),
    };

    // Scrolling is free: the cursor is only followed right after it moved.
    if app.reveal_cursor {
        match app.page_focus {
            PagePane::Sidebar => ensure_visible(
                &mut app.sidebar_scroll,
                &link_offsets(&sidebar),
                app.tag_cursor,
                sidebar_area.height,
            ),
            PagePane::Content => ensure_visible(
                &mut app.content_scroll,
                &link_offsets(&content),
                app.link_cursor,
                content_area.height,
            ),
        }
        app.reveal_cursor = false;
    }

    let sidebar_max = clamp_scroll(
        &mut app.sidebar_scroll,
        total_height(&sidebar),
        sidebar_area.height,
    );
    let content_max = clamp_scroll(
        &mut app.content_scroll,
        total_height(&content),
        content_area.height,
    );

    draw_divider(f, divider_area);
    app.page_links.clear();
    let sidebar_cursor = app.tag_cursor;
    let content_cursor = app.link_cursor;
    let sidebar_hit = draw_items(
        f,
        app,
        &sidebar,
        sidebar_area,
        app.sidebar_scroll,
        sidebar_cursor,
    );
    let content_hit = draw_items(
        f,
        app,
        &content,
        content_area,
        app.content_scroll,
        content_cursor,
    );
    // Load the next credit page when the tab is close to the end.
    if app.tab != PageTab::Overview
        && app.content_scroll + content_area.height + 5 >= total_height(&content)
    {
        app.prefetch_credits();
    }

    app.cursor_visible = match app.page_focus {
        PagePane::Sidebar => sidebar_hit,
        PagePane::Content => content_hit,
    };

    draw_bottom_bar(f, app, sidebar_max, content_max, bottom);
}

/// `(top, height)` of every link, in render order.
fn link_offsets(items: &[Item]) -> Vec<(u16, u16)> {
    let mut y: u16 = 0;
    let mut offsets = Vec::new();
    for item in items {
        let height = item.height();
        for _ in 0..item.link_count() {
            offsets.push((y, height));
        }
        y = y.saturating_add(height);
    }
    offsets
}

fn ensure_visible(scroll: &mut u16, offsets: &[(u16, u16)], cursor: Option<usize>, height: u16) {
    let Some(cursor) = cursor else { return };
    let Some(&(top, link_height)) = offsets.get(cursor) else {
        return;
    };
    if top < *scroll {
        *scroll = top;
    } else if top + link_height > *scroll + height {
        *scroll = (top + link_height).saturating_sub(height);
    }
}

fn clamp_scroll(scroll: &mut u16, total: u16, height: u16) -> u16 {
    let max = total.saturating_sub(height);
    *scroll = (*scroll).min(max);
    max
}

fn draw_tabs(f: &mut Frame, app: &mut App, area: Rect) {
    // AniList's `pageInfo.total` is unreliable for characters and staff (it
    // reports 500 even for series with a dozen entries), so the tabs show no
    // count at all.
    let tabs = [
        (PageTab::Overview, "Overview".to_string()),
        (PageTab::Characters, "Characters".to_string()),
        (PageTab::Staff, "Staff".to_string()),
    ];

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut x = area.x;
    for (tab, label) in tabs {
        let active = app.tab == tab;
        let style = if active {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        let text = format!(" {label} ");
        let width = text.chars().count() as u16;
        if x + width <= area.right() {
            app.tab_targets.push((Rect::new(x, area.y, width, 1), tab));
        }
        spans.push(Span::styled(text, style));
        spans.push(dim(" "));
        x += width + 1;
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn credit_items(
    list: &CreditList,
    title: &str,
    _width: u16,
    kind: Option<crate::tui::model::PersonKind>,
) -> Vec<Item> {
    let mut items = vec![Item::Label(format!(
        "{title} · {} loaded",
        list.items.len()
    ))];

    if list.items.is_empty() {
        items.push(Item::Text(if list.loading {
            "loading…".to_string()
        } else {
            "—".to_string()
        }));
        return items;
    }

    for credit in &list.items {
        let mut role = credit.role.as_deref().map(title_case).unwrap_or_default();
        if !credit.voice_actors.is_empty() {
            if !role.is_empty() {
                role.push_str(" · ");
            }
            role.push_str("VA: ");
            role.push_str(&credit.voice_actors.join(", "));
        }
        items.push(Item::Person {
            name: credit.name.clone(),
            role,
            image: credit.image.clone(),
            link: match (kind, credit.id) {
                (Some(crate::tui::model::PersonKind::Staff), Some(id)) => Some(PageLink::Staff(id)),
                (Some(crate::tui::model::PersonKind::Character), Some(id)) => {
                    Some(PageLink::Character(id))
                }
                _ => None,
            },
        });
    }

    if list.loading {
        items.push(Item::Gap);
        items.push(Item::Label("loading more…".to_string()));
    }

    items
}

fn sidebar_items(app: &App, item: &Media, width: u16) -> Vec<Item> {
    let mut items: Vec<Item> = vec![Item::Cover];

    let entry = app.list_entry();
    let local = (item.kind == MediaKind::Anime)
        .then(|| app.local_progress(item.id))
        .flatten();
    if entry.is_some() || local.is_some() {
        items.extend(list_rows(entry, local, item.episodes));
    }

    for row in info_rows(item) {
        items.push(row);
    }

    if !item.tags.is_empty() {
        let mut tags: Vec<&crate::tui::model::Tag> = item.tags.iter().collect();
        tags.sort_by_key(|tag| std::cmp::Reverse(tag.rank));

        let spoilers = tags.iter().filter(|tag| tag.is_media_spoiler).count();
        let visible: Vec<(String, bool)> = tags
            .iter()
            .filter(|tag| app.show_spoiler_tags || !tag.is_media_spoiler)
            .map(|tag| (tag.name.clone(), tag.is_media_spoiler))
            .collect();

        items.push(Item::Gap);
        items.push(Item::Label("Tags".to_string()));

        if spoilers > 0 {
            let noun = if spoilers == 1 {
                "spoiler tag"
            } else {
                "spoiler tags"
            };
            items.push(Item::Label(if app.show_spoiler_tags {
                format!("s hides {spoilers} {noun}")
            } else {
                format!("{spoilers} {noun} hidden · s to show")
            }));
        }

        for line in pack_tags(&visible, width as usize) {
            items.push(Item::Chips(
                line.into_iter()
                    .map(|(text, spoiler)| Chip {
                        link: PageLink::Tag(text.clone()),
                        text,
                        color: if spoiler { Color::Red } else { Color::Cyan },
                    })
                    .collect(),
            ));
        }
    }

    items
}

/// Greedy packing of tags into full lines, keeping the spoiler flag per tag.
fn pack_tags(tags: &[(String, bool)], width: usize) -> Vec<Vec<(String, bool)>> {
    if width == 0 {
        return Vec::new();
    }

    let mut lines: Vec<Vec<(String, bool)>> = Vec::new();
    let mut current: Vec<(String, bool)> = Vec::new();
    let mut used = 0usize;

    for (name, spoiler) in tags {
        let name = truncate(name, width);
        let name_len = name.chars().count();
        let extra = if current.is_empty() {
            name_len
        } else {
            name_len + 3
        };

        if !current.is_empty() && used + extra > width {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }

        used += if current.is_empty() {
            name_len
        } else {
            name_len + 3
        };
        current.push((name, *spoiler));
    }

    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn list_rows(
    entry: Option<&MediaListEntry>,
    local_progress: Option<i64>,
    episodes: Option<i64>,
) -> Vec<Item> {
    let mut rows = Vec::new();
    let list = Color::Cyan;
    let remote = entry.and_then(|entry| entry.progress).unwrap_or(0);
    // Local progress wins when it is ahead: it is what the user actually
    // watched, even if the sync has not reached AniList yet.
    let progress = local_progress.unwrap_or(remote).max(remote);

    if let Some(status) = entry.and_then(|entry| entry.status.as_deref()) {
        rows.push(Item::Row {
            label: "My Status",
            value: list_status_label(status),
            color: list,
        });
    }
    if let Some(score) = entry.and_then(|entry| entry.score.filter(|score| *score > 0.0)) {
        rows.push(Item::Row {
            label: "My Score",
            value: if score.fract() == 0.0 {
                format!("{score:.0}")
            } else {
                format!("{score:.1}")
            },
            color: list,
        });
    }
    if progress > 0 {
        let value = match episodes.filter(|episodes| *episodes > 0) {
            Some(total) => format!("{progress} / {total}"),
            None => progress.to_string(),
        };
        rows.push(Item::Row {
            label: "Progress",
            value,
            color: if episodes == Some(progress) {
                Color::Green
            } else {
                list
            },
        });
    }
    if let Some(repeat) = entry.and_then(|entry| entry.repeat.filter(|repeat| *repeat > 0)) {
        rows.push(Item::Row {
            label: "Rewatched",
            value: repeat.to_string(),
            color: list,
        });
    }
    if let Some(notes) = entry
        .and_then(|entry| entry.notes.as_deref())
        .map(|notes| notes.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|notes| !notes.is_empty())
    {
        rows.push(Item::Row {
            label: "Notes",
            value: notes,
            color: Color::Gray,
        });
    }

    rows
}

fn list_status_label(status: &str) -> String {
    match status {
        "CURRENT" => "Watching".to_string(),
        "REPEATING" => "Rewatching".to_string(),
        "PLANNING" => "Planning".to_string(),
        "COMPLETED" => "Completed".to_string(),
        "PAUSED" => "Paused".to_string(),
        "DROPPED" => "Dropped".to_string(),
        other => title_case(other),
    }
}

fn info_rows(item: &Media) -> Vec<Item> {
    let mut rows: Vec<Item> = Vec::new();
    let mut push = |label: &'static str, value: Option<String>, color: Color| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            rows.push(Item::Row {
                label,
                value,
                color,
            });
        }
    };
    let plain = Color::Gray;

    push("Format", item.format.clone(), plain);
    match item.kind {
        MediaKind::Anime => push(
            "Episodes",
            item.episodes.map(|episodes| episodes.to_string()),
            plain,
        ),
        MediaKind::Manga => {
            push(
                "Chapters",
                item.chapters.map(|chapters| chapters.to_string()),
                plain,
            );
            push(
                "Volumes",
                item.volumes.map(|volumes| volumes.to_string()),
                plain,
            );
        }
    }
    push(
        "Duration",
        item.duration.map(|mins| format!("{mins} min")),
        plain,
    );
    push(
        "Status",
        item.status.as_deref().map(status_label),
        Color::Magenta,
    );
    push("Start Date", date_label(&item.start_date), plain);
    push("End Date", date_label(&item.end_date), plain);
    push(
        "Season",
        season_label(item.season.as_deref(), item.season_year),
        plain,
    );
    push(
        "Average Score",
        item.average_score.map(|score| format!("{score} / 100")),
        item.average_score.map(score_color).unwrap_or(plain),
    );
    push(
        "Mean Score",
        item.mean_score.map(|score| format!("{score} / 100")),
        item.mean_score.map(score_color).unwrap_or(plain),
    );
    push("Popularity", item.popularity.map(grouped), plain);
    push("Favourites", item.favourites.map(grouped), plain);
    push("Source", item.source.as_deref().map(title_case), plain);
    push(
        "Country",
        item.country_of_origin.as_deref().map(country_label),
        plain,
    );
    push("Studio", studio_names(&item.studios.nodes), plain);
    push("Producers", studio_names(&item.producers.nodes), plain);
    push("Hashtag", item.hashtag.clone(), plain);
    push("Synonyms", join(&item.synonyms), plain);

    rows
}

fn content_items(app: &App, item: &Media, width: u16) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();

    for line in wrap_text(item.title(), width as usize) {
        items.push(Item::Strong(line));
    }

    let mut facts: Vec<String> = Vec::new();
    if let Some(format) = &item.format {
        facts.push(format.clone());
    }
    if let Some(year) = item.season_year {
        facts.push(year.to_string());
    }
    if let Some(episodes) = item.episodes {
        facts.push(format!("{episodes} episodes"));
    }
    if let Some(score) = item.average_score {
        facts.push(format!("★ {score}"));
    }
    items.push(Item::Text(truncate(&facts.join(" · "), width as usize)));

    if item.genres.is_empty() {
        items.push(Item::Text("no genres".to_string()));
    } else {
        for line in pack_tags(
            &item
                .genres
                .iter()
                .map(|genre| (genre.clone(), false))
                .collect::<Vec<_>>(),
            width as usize,
        ) {
            items.push(Item::Chips(
                line.into_iter()
                    .map(|(text, _)| Chip {
                        text: text.clone(),
                        link: PageLink::Genre(text),
                        color: Color::Cyan,
                    })
                    .collect(),
            ));
        }
    }

    items.push(Item::Gap);
    items.push(Item::Rule);
    items.push(Item::Gap);

    items.push(Item::Label("SYNOPSIS".to_string()));
    let description = item
        .description
        .as_deref()
        .map(plain_text)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "No synopsis available.".to_string());
    for line in wrap_text(&description, width as usize) {
        items.push(Item::Text(line));
    }

    let detail = app.detail_data.as_ref();
    let relations = detail.map(|data| data.relations.len()).unwrap_or(0);
    let recommendations = detail.map(|data| data.recommendations.len()).unwrap_or(0);

    items.push(Item::Gap);
    items.push(Item::Label(format!("RELATIONS ({relations})")));
    match detail {
        Some(data) if data.relations.is_empty() => items.push(Item::Text("none".to_string())),
        Some(data) => {
            for relation in &data.relations {
                items.push(Item::Entry {
                    prefix: title_case(&relation.relation_type),
                    media: Box::new(relation.media.clone()),
                });
            }
        }
        None => items.push(Item::Text(page_state(app))),
    }

    if let Some(data) = detail {
        if !data.characters.is_empty() {
            items.push(Item::Gap);
            items.push(Item::Label(format!(
                "CHARACTERS ({})",
                data.characters.len()
            )));
            for credit in &data.characters {
                items.push(Item::Person {
                    name: credit.name.clone(),
                    role: credit.role.as_deref().map(title_case).unwrap_or_default(),
                    image: credit.image.clone(),
                    link: credit.id.map(PageLink::Character),
                });
            }
        }

        if !data.staff.is_empty() {
            items.push(Item::Gap);
            items.push(Item::Label(format!("STAFF ({})", data.staff.len())));
            for credit in &data.staff {
                items.push(Item::Person {
                    name: credit.name.clone(),
                    role: credit.role.clone().unwrap_or_default(),
                    image: credit.image.clone(),
                    link: credit.id.map(PageLink::Staff),
                });
            }
        }
    }

    items.push(Item::Gap);
    items.push(Item::Label(format!("RECOMMENDATIONS ({recommendations})")));
    match detail {
        Some(data) if data.recommendations.is_empty() => items.push(Item::Text("none".to_string())),
        Some(data) => {
            for media in &data.recommendations {
                items.push(Item::Entry {
                    prefix: "→".to_string(),
                    media: Box::new(media.clone()),
                });
            }
        }
        None => items.push(Item::Text(page_state(app))),
    }
    if let Some(error) = &app.detail_error {
        items.push(Item::Gap);
        items.push(Item::Warn(error.clone()));
    }

    items
}

fn page_state(app: &App) -> String {
    if app.page_loading() {
        "loading…".to_string()
    } else {
        "—".to_string()
    }
}

/// Virtual list: items are laid out from the scroll offset, so only what fits
/// on screen is drawn. Every entry registers its rect as a click target.
fn draw_items(
    f: &mut Frame,
    app: &mut App,
    items: &[Item],
    area: Rect,
    scroll: u16,
    cursor: Option<usize>,
) -> bool {
    let mut cursor_drawn = false;
    let mut y = area.y as i32 - scroll as i32;
    let mut entry_index = 0usize;
    for item in items {
        let height = item.height();
        let top = y;
        y += height as i32;

        // Count links even when off screen: the cursor indexes all of them.
        let first_link = entry_index;
        entry_index += item.link_count();

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

        match item {
            Item::Gap => {}
            Item::Cover => draw_cover(f, app, rect),
            Item::Rule => {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        "─".repeat(rect.width as usize),
                        Style::new().fg(Color::DarkGray),
                    )),
                    rect,
                );
            }
            Item::Label(text) => {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        truncate(text, rect.width as usize),
                        Style::new().fg(Color::DarkGray),
                    )),
                    rect,
                );
            }
            Item::Text(text) => {
                f.render_widget(
                    Paragraph::new(Span::styled(text.clone(), Style::new().fg(Color::Gray))),
                    rect,
                );
            }
            Item::Strong(text) => {
                f.render_widget(
                    Paragraph::new(Span::styled(
                        text.clone(),
                        Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
                    )),
                    rect,
                );
            }
            Item::Warn(text) => {
                f.render_widget(
                    Paragraph::new(Span::styled(text.clone(), Style::new().fg(Color::Red))),
                    rect,
                );
            }
            Item::Row {
                label,
                value,
                color,
            } => {
                let value_width = (rect.width as usize).saturating_sub(LABEL_WIDTH);
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            format!("{label:<LABEL_WIDTH$}"),
                            Style::new().fg(Color::DarkGray),
                        ),
                        Span::styled(truncate(value, value_width), Style::new().fg(*color)),
                    ])),
                    rect,
                );
            }
            Item::Chips(chips) => {
                let mut spans: Vec<Span<'static>> = Vec::new();
                let mut x = rect.x;
                for (i, chip) in chips.iter().enumerate() {
                    if i > 0 {
                        if x + 3 > rect.right() {
                            break;
                        }
                        spans.push(dim(" · "));
                        x += 3;
                    }
                    let width = (chip.text.chars().count() as u16).min(rect.right() - x);
                    if width == 0 {
                        break;
                    }
                    let focused = cursor == Some(first_link + i);
                    if focused {
                        cursor_drawn = true;
                    }
                    let style = if focused {
                        Style::new()
                            .fg(Color::Black)
                            .bg(chip.color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(chip.color)
                    };
                    spans.push(Span::styled(chip.text.clone(), style));
                    app.page_links
                        .push((Rect::new(x, rect.y, width, 1), chip.link.clone()));
                    x += width;
                }
                f.render_widget(Paragraph::new(Line::from(spans)), rect);
            }
            Item::Entry { prefix, media } => {
                let focused = cursor == Some(first_link);
                if focused {
                    cursor_drawn = true;
                }
                draw_entry(f, app, prefix, media, rect, height, focused);
            }
            Item::Person {
                name,
                role,
                image,
                link,
            } => {
                let focused = link.is_some() && cursor == Some(first_link);
                if focused {
                    cursor_drawn = true;
                }
                if let Some(link) = link {
                    app.page_links.push((rect, link.clone()));
                }
                let key = match link.as_ref() {
                    Some(PageLink::Staff(id)) => format!("staff:{id}:portrait"),
                    Some(PageLink::Character(id)) => format!("character:{id}:portrait"),
                    _ => format!("portrait:{}", image.as_deref().unwrap_or_default()),
                };
                let image = image.as_deref().map(|url| (url, key.as_str()));
                draw_person(f, app, (name, role), image, rect, height, focused);
            }
        }
    }

    cursor_drawn
}

fn draw_cover(f: &mut Frame, app: &mut App, rect: Rect) {
    let Some(item) = app.page_media() else {
        return;
    };
    let kind = item.kind.label();
    let key = if item.cover_image.extra_large.is_some() {
        format!("{kind}:{}:cover:large", item.id)
    } else {
        format!("{kind}:{}:cover", item.id)
    };
    let proto = item
        .cover_url_large()
        .and_then(|url| app.cover(&key, url, PAGE_COVER.0, PAGE_COVER.1));

    match proto {
        Some(proto) => {
            let size = proto.size();
            f.render_widget(
                Image::new(&proto),
                Rect::new(
                    rect.x,
                    rect.y,
                    size.width.min(rect.width),
                    size.height.min(rect.height),
                ),
            );
        }
        None => {
            f.render_widget(Paragraph::new(dim("···")), rect);
        }
    }
}

fn draw_entry(
    f: &mut Frame,
    app: &mut App,
    prefix: &str,
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
    if text_width > 4 {
        let marker = if focused { "▸ " } else { "  " };
        let title = format!("{marker}{prefix}  {}", media.title());
        let style = if focused {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED)
        };
        f.render_widget(
            Paragraph::new(Span::styled(truncate(&title, text_width as usize), style)),
            Rect::new(text_x, rect.y, text_width, 1),
        );

        if rect.height >= 2 {
            let mut facts: Vec<String> = Vec::new();
            if let Some(format) = &media.format {
                facts.push(format.clone());
            }
            if let Some(year) = media.season_year {
                facts.push(year.to_string());
            }
            if let Some(score) = media.average_score {
                facts.push(format!("★{score}"));
            }
            if let Some(episodes) = media.episodes {
                facts.push(format!("{episodes} ep"));
            }
            f.render_widget(
                Paragraph::new(truncate(&facts.join(" · "), text_width as usize))
                    .style(Style::new().fg(Color::Gray)),
                Rect::new(text_x, rect.y + 1, text_width, 1),
            );
        }
    }

    app.page_links
        .push((rect, PageLink::Media(Box::new(media.clone()))));
}

fn draw_person(
    f: &mut Frame,
    app: &mut App,
    (name, role): (&str, &str),
    image: Option<(&str, &str)>,
    rect: Rect,
    full_height: u16,
    focused: bool,
) {
    let thumb_width = CREDIT_COVER.0.min(rect.width);
    let thumb_height = full_height.min(rect.height);
    let thumb_area = Rect::new(rect.x, rect.y, thumb_width, thumb_height);

    match image.and_then(|(url, key)| app.cover(key, url, CREDIT_COVER.0, CREDIT_COVER.1)) {
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

    let text_x = rect.x + CREDIT_COVER.0 + 1;
    let text_width = rect.right().saturating_sub(text_x);
    if text_width > 4 {
        let style = if focused {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::White)
        };
        f.render_widget(
            Paragraph::new(Span::styled(truncate(name, text_width as usize), style)),
            Rect::new(text_x, rect.y, text_width, 1),
        );
        if rect.height >= 2 {
            f.render_widget(
                Paragraph::new(truncate(role, text_width as usize))
                    .style(Style::new().fg(Color::DarkGray)),
                Rect::new(text_x, rect.y + 1, text_width, 1),
            );
        }
    }
}

fn draw_divider(f: &mut Frame, area: Rect) {
    let line = "│\n".repeat(area.height as usize);
    f.render_widget(
        Paragraph::new(Span::styled(line, Style::new().fg(Color::DarkGray))),
        area,
    );
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }

    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let mut line = String::new();
        for word in raw.split_whitespace() {
            if word.chars().count() > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let chars: Vec<char> = word.chars().collect();
                for chunk in chars.chunks(width) {
                    out.push(chunk.iter().collect());
                }
                continue;
            }

            let needed =
                line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
            if needed > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }

        if !line.is_empty() {
            out.push(line);
        } else if raw.trim().is_empty() {
            // Preserve blank lines from the source text.
            out.push(String::new());
        }
    }
    out
}

fn studio_names(studios: &[crate::tui::model::Studio]) -> Option<String> {
    if studios.is_empty() {
        return None;
    }
    Some(
        studios
            .iter()
            .map(|studio| studio.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn join(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join(", "))
}

fn date_label(date: &crate::tui::model::FuzzyDate) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let year = date.year?;
    match date.month {
        Some(month) => {
            let name = MONTHS
                .get(month.saturating_sub(1) as usize)
                .copied()
                .unwrap_or("?");
            match date.day {
                Some(day) => Some(format!("{name} {day}, {year}")),
                None => Some(format!("{name} {year}")),
            }
        }
        None => Some(year.to_string()),
    }
}

fn season_label(season: Option<&str>, year: Option<i64>) -> Option<String> {
    let season = season.map(title_case)?;
    Some(match year {
        Some(year) => format!("{season} {year}"),
        None => season,
    })
}

fn title_case(value: &str) -> String {
    value
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn country_label(code: &str) -> String {
    match code {
        "JP" => "Japan".into(),
        "CN" => "China".into(),
        "KR" => "South Korea".into(),
        "TW" => "Taiwan".into(),
        "US" => "United States".into(),
        other => other.to_string(),
    }
}

fn status_label(status: &str) -> String {
    match status {
        "FINISHED" => "Finished".into(),
        "RELEASING" => "Releasing".into(),
        "NOT_YET_RELEASED" => "Not yet released".into(),
        "CANCELLED" => "Cancelled".into(),
        "HIATUS" => "Hiatus".into(),
        other => title_case(other),
    }
}

fn score_color(score: i64) -> Color {
    match score {
        75.. => Color::Green,
        60..=74 => Color::Yellow,
        _ => Color::Red,
    }
}

/// 102940 -> "102,940"
fn grouped(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if value < 0 { format!("-{out}") } else { out }
}

fn draw_top_bar(f: &mut Frame, app: &App, id: &i64, area: Rect) {
    let left = Line::from(vec![
        Span::styled(
            " ◂ Esc/⌫ ",
            Style::new().fg(Color::Black).bg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled("aniscrobble", Style::new().fg(Color::Cyan)),
        dim(format!("  anilist.co/anime/{id}")),
    ]);
    f.render_widget(Paragraph::new(left), area);

    let (focus, other) = match app.page_focus {
        PagePane::Sidebar => ("sidebar", "content"),
        PagePane::Content => ("content", "sidebar"),
    };
    let right = Line::from(vec![
        Span::styled(
            format!(" {focus} "),
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        dim(format!(" Tab → {other} ")),
    ]);
    f.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
}

fn draw_bottom_bar(f: &mut Frame, app: &App, sidebar_max: u16, content_max: u16, area: Rect) {
    let left = Line::from(dim(
        " h/l tab · Tab pane · j/k or ↑↓ scroll · g/G top/bottom · Esc/⌫ back",
    ));
    f.render_widget(Paragraph::new(left), area);

    let (scroll, max) = match app.page_focus {
        PagePane::Sidebar => (app.sidebar_scroll, sidebar_max),
        PagePane::Content => (app.content_scroll, content_max),
    };
    let depth = app.history_depth();
    let back = if depth > 0 {
        format!("‹{depth} · ")
    } else {
        String::new()
    };
    let position = if max > 0 {
        format!(
            "{back}{scroll}/{max} · {}/{} ",
            app.selected.saturating_add(1),
            app.items.len()
        )
    } else {
        format!(
            "{back}{}/{} ",
            app.selected.saturating_add(1),
            app.items.len()
        )
    };
    f.render_widget(
        Paragraph::new(dim(position)).alignment(Alignment::Right),
        area,
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Size;
    use ratatui::style::Color;
    use ratatui_image::Resize;
    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc;

    use super::{
        Item, date_label, grouped, list_rows, score_color, season_label, status_label, wrap_text,
    };
    use crate::tui::app::{App, PAGE_COVER, PageLink, PagePane, PageTab};
    use crate::tui::model::{
        CoverImage, FuzzyDate, Media, MediaDetail, Relation, Studio, Studios, Tag, Title,
    };

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

    fn text_of(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[tokio::test]
    async fn page_renders_every_field_and_all_tags() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker.clone(), tx);

        let url = "https://example.test/cover.png";
        let media = Media {
            id: 42,
            title: Title {
                romaji: Some("Test Show".into()),
                english: None,
            },
            cover_image: CoverImage {
                large: Some(url.into()),
                extra_large: Some(url.into()),
            },
            format: Some("TV".into()),
            season_year: Some(2026),
            episodes: Some(12),
            duration: Some(24),
            status: Some("FINISHED".into()),
            season: Some("FALL".into()),
            average_score: Some(85),
            mean_score: Some(80),
            popularity: Some(102940),
            favourites: Some(2829),
            source: Some("ORIGINAL".into()),
            country_of_origin: Some("JP".into()),
            hashtag: Some("#test".into()),
            synonyms: vec!["Alternate Name".into()],
            genres: vec!["Action".into(), "Drama".into()],
            tags: vec![
                Tag {
                    name: "Isekai".into(),
                    rank: 90,
                    is_media_spoiler: false,
                },
                Tag {
                    name: "Spoiler Tag".into(),
                    rank: 50,
                    is_media_spoiler: true,
                },
            ],
            studios: Studios {
                nodes: vec![Studio {
                    name: "TRIGGER".into(),
                }],
            },
            description: Some("<p>A story about tests.</p>".into()),
            start_date: FuzzyDate {
                year: Some(2026),
                month: Some(10),
                day: Some(25),
            },
            ..Default::default()
        };
        app.set_items_for_test(vec![media]);

        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([255, 0, 0, 255]),
        ));
        let proto = picker
            .new_protocol(
                image,
                Size::new(PAGE_COVER.0, PAGE_COVER.1),
                Resize::Fit(None),
            )
            .expect("protocol");
        app.set_cover_for_test(url, PAGE_COVER.0, PAGE_COVER.1, Arc::new(proto));

        let mut terminal = Terminal::new(TestBackend::new(150, 48)).expect("terminal");
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let text = text_of(&terminal);

        // Cover sits in the top left corner of the sidebar, below the top bar.
        assert_ne!(buffer[(0, 1)].symbol(), " ");

        for needle in [
            "Test Show",
            "TV · 2026",
            "Format",
            "Episodes",
            "12",
            "Duration",
            "24 min",
            "Status",
            "Finished",
            "Start Date",
            "Oct 25, 2026",
            "Season",
            "Fall 2026",
            "Average Score",
            "85 / 100",
            "Mean Score",
            "80 / 100",
            "Popularity",
            "102,940",
            "Favourites",
            "2,829",
            "Source",
            "Original",
            "Country",
            "Japan",
            "Studio",
            "TRIGGER",
            "Hashtag",
            "#test",
            "Synonyms",
            "Alternate Name",
            "Tags",
            "Isekai",
            "1 spoiler tag hidden",
            "SYNOPSIS",
            "A story about tests.",
        ] {
            assert!(text.contains(needle), "page is missing {needle:?}");
        }
        assert!(
            !text.contains("Spoiler Tag"),
            "spoiler tags are hidden by default"
        );
    }

    #[tokio::test]
    async fn spoiler_tags_toggle_with_s_and_render_red() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let media = Media {
            tags: vec![
                Tag {
                    name: "Safe Tag".into(),
                    rank: 90,
                    is_media_spoiler: false,
                },
                Tag {
                    name: "Spoiler Tag".into(),
                    rank: 50,
                    is_media_spoiler: true,
                },
            ],
            ..sample("Tagged", 1)
        };
        app.set_items_for_test(vec![media]);
        app.detail = true;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");
        let text = text_of(&terminal);
        assert!(text.contains("Safe Tag"));
        assert!(!text.contains("Spoiler Tag"), "hidden by default");

        app.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('s'),
            KeyModifiers::NONE,
        ));
        assert!(app.show_spoiler_tags);
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");
        let text = text_of(&terminal);
        assert!(text.contains("Spoiler Tag"));
        assert!(!text.contains("Spoiler Tag*"), "no asterisk marker");

        let buffer = terminal.backend().buffer();
        let cell = |needle: &str| {
            let at = text.find(needle).expect("needle rendered");
            let mut bytes = 0;
            for cell in buffer.content() {
                if bytes == at {
                    return cell;
                }
                bytes += cell.symbol().len();
            }
            panic!("cell not found");
        };

        assert_eq!(cell("Spoiler Tag").fg, Color::Red, "spoilers are red");
        assert_eq!(cell("Safe Tag").fg, Color::Cyan);
    }

    #[tokio::test]
    async fn page_lists_relations_and_recommendations_with_thumbnails() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let main = sample("Main Show", 1);
        app.set_items_for_test(vec![main.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: main,
            relations: vec![
                Relation {
                    relation_type: "PREQUEL".into(),
                    media: Media {
                        season_year: Some(2020),
                        ..sample("Earlier Show", 2)
                    },
                },
                Relation {
                    relation_type: "SIDE_STORY".into(),
                    media: sample("Side Show", 3),
                },
            ],
            recommendations: vec![Media {
                format: Some("TV".into()),
                season_year: Some(2021),
                ..sample("Suggested Show", 4)
            }],
            ..Default::default()
        });

        let text = render(&mut app, 140, 50);
        for needle in [
            "RELATIONS (2)",
            "Prequel",
            "Earlier Show",
            "Side Story",
            "RECOMMENDATIONS (1)",
            "Suggested Show",
        ] {
            assert!(text.contains(needle), "page is missing {needle:?}");
        }

        let targets = app.page_targets_for_test();
        let ids: Vec<i64> = targets.iter().map(|(_, id)| *id).collect();
        assert!(ids.contains(&2) && ids.contains(&3) && ids.contains(&4));
    }

    #[tokio::test]
    async fn clicking_an_entry_opens_that_media() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let main = sample("Main Show", 1);
        app.set_items_for_test(vec![main.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: main,
            relations: vec![Relation {
                relation_type: "SEQUEL".into(),
                media: sample("Next Show", 9),
            }],
            recommendations: Vec::new(),
            ..Default::default()
        });
        app.detail = true;
        render(&mut app, 140, 50);

        let (rect, id) = app
            .page_targets_for_test()
            .into_iter()
            .find(|(_, id)| *id == 9)
            .expect("target for the relation");

        let before = app.detail_scope_for_test();
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 2,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.page_media().map(|media| media.id), Some(id));
        assert_ne!(app.detail_scope_for_test(), before);
        assert!(app.page_loading(), "a fresh detail request is in flight");
        assert_eq!(app.history_depth(), 1, "the previous page is remembered");

        let backspace = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Backspace,
            KeyModifiers::NONE,
        );
        app.on_key(backspace);
        assert!(app.detail);
        assert_eq!(app.page_media().map(|media| media.id), Some(1));
        assert_eq!(app.history_depth(), 0);

        app.on_key(backspace);
        assert!(!app.detail);
    }

    #[tokio::test]
    async fn panes_scroll_and_clamp_independently() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let tags: Vec<Tag> = (0..40)
            .map(|i| Tag {
                name: format!("Tag{i}"),
                rank: i,
                is_media_spoiler: false,
            })
            .collect();
        let media = Media {
            format: Some("TV".into()),
            tags,
            description: Some("word ".repeat(1200)),
            ..sample("Long", 1)
        };
        app.set_items_for_test(vec![media]);

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("terminal");
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");
        assert!(text_of(&terminal).contains("Format"));

        // Content scrolls: the sidebar keeps its own offset.
        app.content_scroll = u16::MAX;
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");
        let content_max = app.content_scroll;
        assert!(content_max > 0 && content_max < u16::MAX);
        assert!(
            !text_of(&terminal).contains("Tag0"),
            "sidebar did not move: the tags are still below the fold"
        );
        assert!(
            !text_of(&terminal).contains("SYNOPSIS"),
            "content scrolled past the synopsis"
        );

        // Sidebar scrolls on its own, cover included.
        app.sidebar_scroll = u16::MAX;
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");
        let sidebar_max = app.sidebar_scroll;
        assert!(sidebar_max > 0 && sidebar_max < u16::MAX);
        assert!(
            text_of(&terminal).contains("Tag0"),
            "sidebar scrolled down to its tags"
        );
        assert_eq!(app.content_scroll, content_max, "content offset untouched");

        app.page_focus = PagePane::Sidebar;
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");
        assert!(text_of(&terminal).contains("sidebar"), "focus badge");
    }

    #[tokio::test]
    async fn scrolling_stays_free_when_a_link_is_focused() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let main = sample("Main Show", 1);
        app.set_items_for_test(vec![main.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: main,
            relations: Vec::new(),
            recommendations: (0..12)
                .map(|i| sample(&format!("Recommended {i}"), 100 + i))
                .collect(),
            ..Default::default()
        });
        app.detail = true;
        app.page_focus = PagePane::Content;

        let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("terminal");
        let draw = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|frame| super::draw(frame, app, frame.area()))
                .expect("draw");
        };
        draw(&mut app, &mut terminal);
        assert_eq!(app.content_scroll, 0);

        // Moving the cursor brings it into view, once.
        let key = |code| crossterm::event::KeyEvent::new(code, KeyModifiers::NONE);
        app.on_key(key(crossterm::event::KeyCode::Char('n')));
        draw(&mut app, &mut terminal);
        assert!(app.cursor_visible, "first link is on screen");
        assert!(text_of(&terminal).contains("▸"), "focused link marker");

        for _ in 0..11 {
            app.on_key(key(crossterm::event::KeyCode::Char('n')));
        }
        draw(&mut app, &mut terminal);
        assert_eq!(app.link_cursor, Some(11));
        assert!(app.content_scroll > 0, "cursor followed to the bottom");
        assert!(app.cursor_visible);

        // Manual scrolling is never overridden...
        app.content_scroll = 0;
        app.reveal_cursor = false;
        draw(&mut app, &mut terminal);
        assert_eq!(app.content_scroll, 0, "scroll stays free");
        assert!(!app.cursor_visible, "the focused link is now off screen");

        // ...and an off-screen link cannot be opened.
        let before = app.detail_scope_for_test();
        app.on_key(key(crossterm::event::KeyCode::Enter));
        assert_eq!(app.detail_scope_for_test(), before, "Enter did nothing");
        assert_eq!(app.page_media().map(|media| media.id), Some(1));

        // The next `n` continues from the selected link and reveals it again.
        app.on_key(key(crossterm::event::KeyCode::Char('n')));
        draw(&mut app, &mut terminal);
        assert_eq!(app.link_cursor, Some(11), "already the last link");
        assert!(app.cursor_visible);
        assert!(app.content_scroll > 0);
    }

    #[tokio::test]
    async fn characters_tab_lists_every_loaded_credit() {
        use crate::tui::app::{CreditList, PageTab};

        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let main = sample("Main Show", 1);
        app.set_items_for_test(vec![main.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: main,
            ..Default::default()
        });
        app.detail = true;
        app.tab = PageTab::Characters;
        app.characters_tab = CreditList {
            items: (0..5)
                .map(|i| crate::tui::model::Credit {
                    id: Some(1000 + i),
                    name: format!("Character {i}"),
                    role: Some("MAIN".into()),
                    voice_actors: vec![format!("Voice {i}")],
                    image: None,
                })
                .collect(),
            page: 1,
            has_next: false,
            loading: false,
        };

        let text = render(&mut app, 140, 70);
        for needle in [
            "CHARACTERS · 5 loaded",
            "Character 4",
            "Main",
            "VA: Voice 4",
        ] {
            assert!(
                text.contains(needle),
                "characters tab is missing {needle:?}"
            );
        }
        assert!(text.contains("Characters"), "tab bar entry");
        assert!(
            !text.contains("Characters ("),
            "no count in the tab bar: AniList totals are unreliable"
        );
        assert_eq!(
            app.page_links_for_test().len(),
            5,
            "every character row on the tab is a link"
        );
    }

    #[tokio::test]
    async fn clicking_a_tag_or_genre_browses_it() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let media = Media {
            genres: vec!["Action".into()],
            tags: vec![Tag {
                name: "Isekai".into(),
                rank: 90,
                is_media_spoiler: false,
            }],
            ..sample("Main Show", 1)
        };
        app.set_items_for_test(vec![media.clone()]);
        app.detail = true;
        render(&mut app, 120, 40);

        let click = |app: &mut App, needle: &crate::tui::app::PageLink| {
            let rect = app
                .page_links_for_test()
                .into_iter()
                .find(|(_, link)| std::mem::discriminant(link) == std::mem::discriminant(needle))
                .map(|(rect, _)| rect)
                .expect("clickable chip");
            app.on_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            });
        };

        click(&mut app, &crate::tui::app::PageLink::Tag("Isekai".into()));
        assert_eq!(app.source, crate::tui::app::Source::Tag("Isekai".into()));
        assert!(!app.detail, "browsing leaves the page");

        app.set_items_for_test(vec![media]);
        app.detail = true;
        render(&mut app, 120, 40);
        click(&mut app, &crate::tui::app::PageLink::Genre("Action".into()));
        assert_eq!(app.source, crate::tui::app::Source::Genre("Action".into()));
        assert!(!app.detail);
    }

    #[tokio::test]
    async fn keyboard_reaches_genres_before_media_links() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let media = Media {
            genres: vec!["Action".into()],
            ..sample("Main Show", 1)
        };
        app.set_items_for_test(vec![media]);
        app.detail = true;
        app.page_focus = PagePane::Content;

        let key = |code| crossterm::event::KeyEvent::new(code, KeyModifiers::NONE);
        app.on_key(key(crossterm::event::KeyCode::Char('n')));
        assert_eq!(app.link_cursor, Some(0));
        app.on_key(key(crossterm::event::KeyCode::Enter));

        assert_eq!(app.source, crate::tui::app::Source::Genre("Action".into()));
        assert!(!app.detail);
    }

    #[tokio::test]
    async fn page_lists_characters_and_staff() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);

        let main = sample("Main Show", 1);
        app.set_items_for_test(vec![main.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: main,
            list_entry: None,
            relations: Vec::new(),
            characters: vec![
                crate::tui::model::Credit {
                    id: Some(1),
                    name: "D".into(),
                    image: None,
                    role: Some("MAIN".into()),
                    voice_actors: Vec::new(),
                },
                crate::tui::model::Credit {
                    id: Some(2),
                    name: "Roman Carax".into(),
                    image: None,
                    role: Some("SUPPORTING".into()),
                    voice_actors: Vec::new(),
                },
            ],
            staff: vec![crate::tui::model::Credit {
                id: Some(3),
                name: "Mike Pondsmith".into(),
                image: None,
                role: Some("Original Creator".into()),
                voice_actors: Vec::new(),
            }],
            recommendations: vec![sample("Suggested Show", 9)],
        });
        app.detail = true;

        let text = render(&mut app, 140, 70);
        for needle in [
            "CHARACTERS (2)",
            "D",
            "Main",
            "Roman Carax",
            "Supporting",
            "STAFF (1)",
            "Mike Pondsmith",
            "Original Creator",
        ] {
            assert!(text.contains(needle), "page is missing {needle:?}");
        }

        // Overview order: characters and staff come before recommendations.
        let characters_at = text.find("CHARACTERS").expect("characters section");
        let recommendations_at = text
            .find("RECOMMENDATIONS")
            .expect("recommendations section");
        assert!(
            characters_at < recommendations_at,
            "characters must be listed before recommendations"
        );

        let links = app.page_links_for_test();
        assert_eq!(
            links.len(),
            4,
            "two characters, a staff member, a recommendation"
        );
        assert!(
            links
                .iter()
                .any(|(_, link)| matches!(link, PageLink::Staff(3))),
            "staff rows open their own page"
        );
        assert!(
            links
                .iter()
                .any(|(_, link)| matches!(link, PageLink::Character(1))),
            "character rows open their own page"
        );
    }

    #[tokio::test]
    async fn tabs_belong_to_the_content_column() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);
        app.set_items_for_test(vec![sample("Main Show", 1)]);
        app.detail = true;

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("terminal");
        terminal
            .draw(|frame| super::draw(frame, &mut app, frame.area()))
            .expect("draw");

        let width = 120usize;
        let buffer = terminal.backend().buffer();
        let text = text_of(&terminal);
        let at = text.find("Overview").expect("tab bar rendered");

        let mut bytes = 0;
        let mut position = (0usize, 0usize);
        for (index, cell) in buffer.content().iter().enumerate() {
            if bytes == at {
                position = (index % width, index / width);
                break;
            }
            bytes += cell.symbol().len();
        }

        assert!(
            position.0 >= 34,
            "tabs must start inside the content column, got x={}",
            position.0
        );
        assert_eq!(position.1, 1, "tabs sit on the first body row");
        // The sidebar keeps its own first row free for the cover.
        assert_ne!(buffer[(0, 1)].symbol(), " ");
    }

    #[tokio::test]
    async fn tabs_are_clickable_with_the_mouse() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::new(picker, tx);
        app.set_items_for_test(vec![sample("Main Show", 1)]);
        app.detail = true;

        render(&mut app, 120, 30);
        let targets = app.tab_targets_for_test();
        assert_eq!(targets.len(), 3, "three clickable tabs");

        let click = |app: &mut App, tab: PageTab| {
            let (rect, _) = app
                .tab_targets_for_test()
                .into_iter()
                .find(|(_, candidate)| *candidate == tab)
                .expect("tab rect");
            app.on_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 1,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            });
        };

        click(&mut app, PageTab::Characters);
        assert_eq!(app.tab, PageTab::Characters);
        render(&mut app, 120, 30);

        click(&mut app, PageTab::Staff);
        assert_eq!(app.tab, PageTab::Staff);
        render(&mut app, 120, 30);

        click(&mut app, PageTab::Overview);
        assert_eq!(app.tab, PageTab::Overview);
    }

    #[test]
    fn wraps_long_text_and_hard_splits_words() {
        assert_eq!(wrap_text("a b c", 3), vec!["a b", "c"]);
        assert_eq!(wrap_text("abcdefgh", 4), vec!["abcd", "efgh"]);
        assert!(wrap_text("", 10).is_empty());
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(grouped(102940), "102,940");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
    }

    #[test]
    fn score_colors_follow_anilist_ranges() {
        assert_eq!(score_color(84), Color::Green);
        assert_eq!(score_color(65), Color::Yellow);
        assert_eq!(score_color(20), Color::Red);
    }

    #[test]
    fn formats_fuzzy_dates() {
        let full = FuzzyDate {
            year: Some(2026),
            month: Some(10),
            day: Some(25),
        };
        assert_eq!(date_label(&full), Some("Oct 25, 2026".into()));

        let year_only = FuzzyDate {
            year: Some(2026),
            month: None,
            day: None,
        };
        assert_eq!(date_label(&year_only), Some("2026".into()));
        assert_eq!(date_label(&FuzzyDate::default()), None);
    }

    #[test]
    fn labels_seasons_and_statuses() {
        assert_eq!(
            season_label(Some("FALL"), Some(2026)),
            Some("Fall 2026".into())
        );
        assert_eq!(season_label(None, Some(2026)), None);
        assert_eq!(status_label("NOT_YET_RELEASED"), "Not yet released");
    }

    #[tokio::test]
    async fn sidebar_shows_the_viewer_list_entry() {
        let picker = Arc::new(Picker::halfblocks());
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App::with_account(
            picker,
            tx,
            None,
            None,
            None,
            std::collections::HashMap::from([(1, 7)]),
        );
        let main = Media {
            episodes: Some(12),
            ..sample("Main Show", 1)
        };
        app.set_items_for_test(vec![main.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: main,
            list_entry: Some(crate::tui::model::MediaListEntry {
                status: Some("CURRENT".into()),
                score: Some(8.5),
                progress: Some(5),
                repeat: Some(2),
                notes: Some("rewatch with\nfriends".into()),
            }),
            ..Default::default()
        });
        app.detail = true;

        let text = render(&mut app, 140, 70);
        for needle in [
            "My Status",
            "Watching",
            "My Score",
            "8.5",
            "7 / 12",
            "rewatch with",
        ] {
            assert!(text.contains(needle), "page is missing {needle:?}");
        }
    }

    #[test]
    fn local_progress_is_the_truth_when_it_is_ahead() {
        let entry = crate::tui::model::MediaListEntry {
            status: Some("CURRENT".into()),
            progress: Some(3),
            ..Default::default()
        };
        let progress = |entry, local| {
            list_rows(entry, local, Some(12))
                .into_iter()
                .find_map(|item| match item {
                    Item::Row {
                        label: "Progress",
                        value,
                        ..
                    } => Some(value),
                    _ => None,
                })
        };

        assert_eq!(progress(Some(&entry), Some(7)).as_deref(), Some("7 / 12"));
        assert_eq!(progress(Some(&entry), Some(2)).as_deref(), Some("3 / 12"));
        assert_eq!(progress(None, Some(4)).as_deref(), Some("4 / 12"));
        assert_eq!(progress(None, None), None);
    }
}
