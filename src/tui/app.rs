use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use tokio::sync::mpsc;

use crate::tui::model::{
    Credit, CreditKind, Media, MediaDetail, MediaKind, MyListEntry, PersonKind, PersonPageData,
};
use crate::tui::net::{Job, Net, NetMsg, ReqKey, ScopeId};

pub type CoverKey = (String, u16, u16);

/// Cover size, in terminal cells, for one list row.
pub const LIST_COVER: (u16, u16) = (8, 6);
/// Cover size for the full screen media page.
pub const PAGE_COVER: (u16, u16) = (30, 18);
/// Thumbnail size, in terminal cells, for relations and recommendations.
pub const THUMB_COVER: (u16, u16) = (6, 3);
/// Portrait size for characters and staff.
pub const CREDIT_COVER: (u16, u16) = (4, 3);
/// Portrait size on the staff page.
pub const STAFF_PORTRAIT: (u16, u16) = (10, 14);

const PER_PAGE: u32 = 30;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);
/// Rows moved per wheel notch.
const WHEEL_STEP: i64 = 3;
/// Load the next page when the selection gets this close to the last loaded row.
const PREFETCH_AHEAD: usize = 5;
/// Rows per credit page.
const CREDITS_PER_PAGE: u32 = 25;

/// Which column of the media page owns the scroll keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagePane {
    Sidebar,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Trending,
    Search(String),
    Genre(String),
    Tag(String),
    /// The viewer's current anime and manga lists on one page.
    Mine,
}

/// One paginated section of the viewer's current list.
#[derive(Debug, Default)]
pub struct MineSection {
    pub items: Vec<MyListEntry>,
    pub page: u32,
    pub has_next: bool,
    pub loading: bool,
}

#[derive(Debug, Default)]
pub struct MineLists {
    pub anime: MineSection,
    pub manga: MineSection,
}

impl MineLists {
    fn section(&self, kind: MediaKind) -> &MineSection {
        match kind {
            MediaKind::Anime => &self.anime,
            MediaKind::Manga => &self.manga,
        }
    }

    fn section_mut(&mut self, kind: MediaKind) -> &mut MineSection {
        match kind {
            MediaKind::Anime => &mut self.anime,
            MediaKind::Manga => &mut self.manga,
        }
    }

    fn loading(&self) -> bool {
        self.anime.loading || self.manga.loading
    }

    fn has_next(&self) -> bool {
        self.anime.has_next || self.manga.has_next
    }
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Trending => "trending".into(),
            Source::Search(query) => format!("search \"{query}\""),
            Source::Genre(genre) => format!("genre \"{genre}\""),
            Source::Tag(tag) => format!("tag \"{tag}\""),
            Source::Mine => "in progress".into(),
        }
    }
}

/// Page context saved when a genre/tag sends the user back to the list, so
/// `Backspace` can restore the page exactly as it was.
#[derive(Debug, Clone)]
struct BrowseOrigin {
    subject: Media,
    history: Vec<Media>,
}

/// Staff or character page state: profile plus its paginated works.
#[derive(Debug, Clone, Default)]
pub struct PersonView {
    pub data: Option<PersonPageData>,
    pub page: u32,
    pub loading: bool,
    pub error: Option<String>,
    pub scroll: u16,
    /// Keyboard cursor over the collaborations (staff positions, then voice roles).
    pub cursor: Option<usize>,
    pub reveal: bool,
    pub cursor_visible: bool,
}

/// Media page tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTab {
    Overview,
    Characters,
    Staff,
}

/// One paginated credit list (characters or staff).
#[derive(Debug, Clone, Default)]
pub struct CreditList {
    pub items: Vec<Credit>,
    pub page: u32,
    pub has_next: bool,
    pub loading: bool,
}

/// Something clickable on the media page: a media, or a catalog filter.
#[derive(Debug, Clone)]
pub enum PageLink {
    Media(Box<Media>),
    Staff(i64),
    Character(i64),
    Genre(String),
    Tag(String),
}

pub struct App {
    pub running: bool,
    pub dirty: bool,
    pub source: Source,
    pub items: Vec<Media>,
    pub selected: usize,
    pub page: u32,
    pub has_next: bool,
    pub list_loading: bool,
    pub search_input: String,
    pub search_focus: bool,
    pub detail: bool,
    /// Media page focus: sidebar (cover + fields + tags) or content.
    pub page_focus: PagePane,
    /// Active media page tab.
    pub tab: PageTab,
    /// Open staff or character page, if any.
    pub person_view: Option<PersonView>,
    pub characters_tab: CreditList,
    pub staff_tab: CreditList,
    /// Independent scroll offsets of the two page columns, in rows.
    pub sidebar_scroll: u16,
    pub content_scroll: u16,
    /// Keyboard cursor over the content links (genres, relations,
    /// recommendations).
    pub link_cursor: Option<usize>,
    /// Keyboard cursor over the sidebar tags.
    pub tag_cursor: Option<usize>,
    /// Whether the focused cursor link was on screen at the last draw. An
    /// off-screen link cannot be opened.
    pub cursor_visible: bool,
    /// Spoiler tags are hidden until the user asks for them with `s`.
    pub show_spoiler_tags: bool,
    browse_origin: Option<BrowseOrigin>,
    /// Set when the cursor moves: the next draw scrolls it into view once, then
    /// leaves scrolling completely free.
    pub reveal_cursor: bool,
    pub detail_data: Option<MediaDetail>,
    /// Episode reached locally, including progress not synced to AniList yet.
    local_progress: HashMap<i64, i64>,
    /// Progress from the viewer's AniList list, keyed by media id.
    my_progress: HashMap<i64, i64>,
    /// Current anime and manga sections, loaded page by page.
    mine: MineLists,
    /// Both first pages have landed at least once for the current source.
    mine_ready: bool,
    /// Section headers for the combined current-list page: item index -> label.
    list_headers: Vec<(usize, String)>,
    /// Logged-in AniList user id. `None` disables the current list.
    viewer: Option<u64>,
    /// Media currently shown by the page, possibly not loaded in full yet.
    pub page_subject: Option<Media>,
    /// Previously visited pages, most recent last. `Backspace` pops it.
    pub page_history: Vec<Media>,
    /// Clickable page rows and chips. Rebuilt on every draw.
    pub page_links: Vec<(Rect, PageLink)>,
    /// Clickable media page tabs. Rebuilt on every draw.
    pub tab_targets: Vec<(Rect, PageTab)>,
    /// Clickable list rows: screen rect -> media id. Rebuilt on every draw.
    pub list_targets: Vec<(Rect, i64)>,
    pub detail_loading: bool,
    pub detail_error: Option<String>,
    pub error: Option<String>,
    pub spinner: u8,
    pub protocol: String,

    covers: HashMap<CoverKey, Arc<Protocol>>,
    pending_covers: HashSet<CoverKey>,
    failed_covers: HashSet<CoverKey>,
    search_due: Option<Instant>,
    scope: ScopeId,
    detail_scope: Option<ScopeId>,
    person_scope: Option<ScopeId>,
    net: Net,
}

impl App {
    #[cfg(test)]
    pub fn new(picker: Arc<Picker>, tx: mpsc::Sender<NetMsg>) -> Self {
        Self::with_account(picker, tx, None, None, None, HashMap::new())
    }

    pub fn with_account(
        picker: Arc<Picker>,
        tx: mpsc::Sender<NetMsg>,
        db: Option<crate::database::Database>,
        token: Option<String>,
        viewer: Option<u64>,
        local_progress: HashMap<i64, i64>,
    ) -> Self {
        let protocol = format!("{:?}", picker.protocol_type()).to_lowercase();
        let mut net = Net::with_account(picker, tx, token, db);
        let scope = net.mount();

        let source = if viewer.is_some() {
            Source::Mine
        } else {
            Source::Trending
        };
        let mut app = Self {
            running: true,
            dirty: true,
            source,
            items: Vec::new(),
            selected: 0,
            page: 1,
            has_next: true,
            list_loading: false,
            search_input: String::new(),
            search_focus: false,
            detail: false,
            page_focus: PagePane::Content,
            tab: PageTab::Overview,
            person_view: None,
            characters_tab: CreditList::default(),
            staff_tab: CreditList::default(),
            sidebar_scroll: 0,
            content_scroll: 0,
            link_cursor: None,
            tag_cursor: None,
            cursor_visible: true,
            reveal_cursor: false,
            show_spoiler_tags: false,
            browse_origin: None,
            detail_data: None,
            local_progress,
            my_progress: HashMap::new(),
            mine: MineLists::default(),
            mine_ready: false,
            list_headers: Vec::new(),
            viewer,
            page_subject: None,
            page_history: Vec::new(),
            page_links: Vec::new(),
            tab_targets: Vec::new(),
            list_targets: Vec::new(),
            detail_loading: false,
            detail_error: None,
            error: None,
            spinner: 0,
            protocol,
            covers: HashMap::new(),
            pending_covers: HashSet::new(),
            failed_covers: HashSet::new(),
            search_due: None,
            scope,
            detail_scope: None,
            person_scope: None,
            net,
        };
        app.request_page();
        app
    }

    pub fn inflight(&self) -> usize {
        self.net.inflight()
    }

    /// Left click on a page row opens that media. Rows are registered by the
    /// page renderer, so the hit test always matches what is on screen.
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.wheel(-WHEEL_STEP);
                return;
            }
            MouseEventKind::ScrollDown => {
                self.wheel(WHEEL_STEP);
                return;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return,
        }
        let point = Position::new(mouse.column, mouse.row);

        if self.person_open() {
            let link = self
                .page_links
                .iter()
                .find(|(rect, _)| rect.contains(point))
                .map(|(_, link)| link.clone());
            if let Some(link) = link {
                self.follow(link);
            }
            return;
        }

        if self.detail {
            let tab = self
                .tab_targets
                .iter()
                .find(|(rect, _)| rect.contains(point))
                .map(|(_, tab)| *tab);
            if let Some(tab) = tab {
                self.switch_tab(tab);
                return;
            }
        }

        if !self.detail {
            let hit = self
                .list_targets
                .iter()
                .find(|(rect, _)| rect.contains(point))
                .map(|(_, id)| *id);
            if let Some(id) = hit
                && let Some(index) = self.items.iter().position(|media| media.id == id)
            {
                self.selected = index;
                self.page_history.clear();
                self.dirty = true;
                self.open_page();
            }
            return;
        }

        let link = self
            .page_links
            .iter()
            .find(|(rect, _)| rect.contains(point))
            .map(|(_, link)| link.clone());
        if let Some(link) = link {
            self.follow(link);
        }
    }

    /// Act on a page link: open a media page or browse a genre/tag.
    fn follow(&mut self, link: PageLink) {
        match link {
            PageLink::Media(media) => {
                if self.page_media().map(|current| current.id) == Some(media.id) {
                    return;
                }
                // Works are opened from the staff page: close it first, the media
                // page underneath becomes the history entry.
                self.close_person();
                self.open_media(*media);
            }
            PageLink::Staff(id) => self.open_person(PersonKind::Staff, id),
            PageLink::Character(id) => self.open_person(PersonKind::Character, id),
            PageLink::Genre(genre) => self.browse(Source::Genre(genre)),
            PageLink::Tag(tag) => self.browse(Source::Tag(tag)),
        }
    }

    /// Leave the page and show the list for a genre or tag.
    fn browse(&mut self, source: Source) {
        if self.detail
            && let Some(subject) = self.page_media()
        {
            self.browse_origin = Some(BrowseOrigin {
                subject,
                history: std::mem::take(&mut self.page_history),
            });
        }
        self.source = source;
        self.refetch();
    }

    /// Back from a genre/tag list to the page it was opened from.
    fn back_from_browse(&mut self) {
        let Some(origin) = self.browse_origin.take() else {
            return;
        };
        self.page_history = origin.history;
        self.load_page(origin.subject);
    }

    pub fn has_browse_origin(&self) -> bool {
        self.browse_origin.is_some()
    }

    /// Wheel scrolling: the list moves its selection (which drives the
    /// viewport), the media page moves its own scroll offset.
    fn wheel(&mut self, delta: i64) {
        self.dirty = true;
        if let Some(view) = &mut self.person_view {
            view.scroll = (view.scroll as i64 + delta).max(0) as u16;
        } else if self.detail {
            self.scroll_focused(delta);
        } else {
            self.move_selection(delta);
        }
    }

    /// Links reachable from the content column, in render order: genres first,
    /// then relations and recommendations.
    fn content_links(&self) -> Vec<PageLink> {
        // Links follow the active tab: the staff tab lists its own rows.
        if self.tab == PageTab::Staff {
            return self
                .staff_tab
                .items
                .iter()
                .filter_map(|credit| credit.id)
                .map(PageLink::Staff)
                .collect();
        }
        if self.tab == PageTab::Characters {
            return self
                .characters_tab
                .items
                .iter()
                .filter_map(|credit| credit.id)
                .map(PageLink::Character)
                .collect();
        }

        let mut links: Vec<PageLink> = Vec::new();
        if let Some(media) = self.page_media() {
            links.extend(media.genres.iter().cloned().map(PageLink::Genre));
        }
        if let Some(data) = &self.detail_data {
            links.extend(
                data.relations
                    .iter()
                    .map(|relation| PageLink::Media(Box::new(relation.media.clone()))),
            );
            links.extend(
                data.characters
                    .iter()
                    .filter_map(|credit| credit.id)
                    .map(PageLink::Character),
            );
            links.extend(
                data.staff
                    .iter()
                    .filter_map(|credit| credit.id)
                    .map(PageLink::Staff),
            );
            links.extend(
                data.recommendations
                    .iter()
                    .cloned()
                    .map(|media| PageLink::Media(Box::new(media))),
            );
        }
        links
    }

    /// Tags in the sidebar, honoring the spoiler filter, in render order.
    fn tag_links(&self) -> Vec<PageLink> {
        let Some(media) = self.page_media() else {
            return Vec::new();
        };
        let mut tags: Vec<&crate::tui::model::Tag> = media.tags.iter().collect();
        tags.sort_by_key(|tag| std::cmp::Reverse(tag.rank));
        tags.into_iter()
            .filter(|tag| self.show_spoiler_tags || !tag.is_media_spoiler)
            .map(|tag| PageLink::Tag(tag.name.clone()))
            .collect()
    }

    /// Step the cursor of the focused column.
    fn move_link(&mut self, delta: i64) {
        let count = match self.page_focus {
            PagePane::Sidebar => self.tag_links().len(),
            PagePane::Content => self.content_links().len(),
        };
        if count == 0 {
            return;
        }

        let cursor = match self.page_focus {
            PagePane::Sidebar => &mut self.tag_cursor,
            PagePane::Content => &mut self.link_cursor,
        };

        *cursor = match *cursor {
            None if delta > 0 => Some(0),
            None => Some(count - 1),
            Some(index) => {
                let next = index as i64 + delta;
                if next < 0 {
                    None
                } else {
                    Some((next as usize).min(count - 1))
                }
            }
        };
        self.reveal_cursor = true;
        self.dirty = true;
    }

    fn cycle_tab(&mut self, delta: i64) {
        let tabs = [PageTab::Overview, PageTab::Characters, PageTab::Staff];
        let current = tabs.iter().position(|tab| *tab == self.tab).unwrap_or(0) as i64;
        let next = (current + delta).rem_euclid(tabs.len() as i64) as usize;
        self.switch_tab(tabs[next]);
    }

    fn switch_tab(&mut self, tab: PageTab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.content_scroll = 0;
        self.link_cursor = None;
        match tab {
            PageTab::Overview => {}
            PageTab::Characters => self.ensure_credits(CreditKind::Characters),
            PageTab::Staff => self.ensure_credits(CreditKind::Staff),
        }
        self.dirty = true;
    }

    fn credit_list(&mut self, kind: CreditKind) -> &mut CreditList {
        match kind {
            CreditKind::Characters => &mut self.characters_tab,
            CreditKind::Staff => &mut self.staff_tab,
        }
    }

    fn ensure_credits(&mut self, kind: CreditKind) {
        let list = self.credit_list(kind);
        if list.items.is_empty() && !list.loading {
            self.request_credits(kind, 1);
        }
    }

    fn request_credits(&mut self, kind: CreditKind, page: u32) {
        let Some(media) = self.page_media() else {
            return;
        };
        let (id, media_kind) = (media.id, media.kind);
        let Some(scope) = self.detail_scope else {
            return;
        };
        self.credit_list(kind).loading = true;
        self.dirty = true;
        self.net.request(
            scope,
            ReqKey::Credits(kind, page),
            Job::Credits {
                kind,
                media_kind,
                id,
                page,
                per_page: CREDITS_PER_PAGE,
            },
        );
    }

    /// Called by the renderer when the active credit tab is close to the end.
    pub fn prefetch_credits(&mut self) {
        let kind = match self.tab {
            PageTab::Overview => return,
            PageTab::Characters => CreditKind::Characters,
            PageTab::Staff => CreditKind::Staff,
        };
        let list = self.credit_list(kind);
        if list.loading || !list.has_next {
            return;
        }
        let page = list.page + 1;
        self.request_credits(kind, page);
    }

    fn open_person(&mut self, kind: PersonKind, id: i64) {
        self.close_person();
        let scope = self.net.mount();
        self.person_scope = Some(scope);
        self.person_view = Some(PersonView {
            loading: true,
            cursor_visible: true,
            ..Default::default()
        });
        self.request_person(kind, id, 1);
    }

    fn request_person(&mut self, kind: PersonKind, id: i64, page: u32) {
        let Some(scope) = self.person_scope else {
            return;
        };
        if let Some(view) = &mut self.person_view {
            view.loading = true;
        }
        self.dirty = true;
        self.net.request(
            scope,
            ReqKey::Person(kind, id, page),
            Job::Person {
                kind,
                id,
                page,
                per_page: CREDITS_PER_PAGE,
            },
        );
    }

    fn close_person(&mut self) {
        self.person_view = None;
        self.dirty = true;
        if let Some(scope) = self.person_scope.take() {
            self.net.retire(scope);
        }
    }

    pub fn person_open(&self) -> bool {
        self.person_view.is_some()
    }

    /// Called by the person page renderer when near the end of the list.
    pub fn prefetch_person(&mut self) {
        let Some(view) = &self.person_view else {
            return;
        };
        if view.loading {
            return;
        }
        let Some(data) = &view.data else {
            return;
        };
        if !data.has_next {
            return;
        }
        let page = view.page + 1;
        let kind = data.kind;
        let id = data.id;
        self.request_person(kind, id, page);
    }

    /// Enter on the focused link.
    fn activate_link(&mut self) {
        if !self.cursor_visible {
            // Scrolled away: the link is still selected but cannot be opened.
            return;
        }
        let link = match self.page_focus {
            PagePane::Sidebar => self
                .tag_cursor
                .and_then(|index| self.tag_links().get(index).cloned()),
            PagePane::Content => self
                .link_cursor
                .and_then(|index| self.content_links().get(index).cloned()),
        };
        if let Some(link) = link {
            self.follow(link);
        }
    }

    /// Move the scroll of the focused page column. The upper bound is enforced
    /// at draw time, where the content height is known.
    fn scroll_focused(&mut self, delta: i64) {
        let target = match self.page_focus {
            PagePane::Sidebar => &mut self.sidebar_scroll,
            PagePane::Content => &mut self.content_scroll,
        };
        *target = (*target as i64 + delta).max(0) as u16;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.running = false;
            return;
        }

        self.dirty = true;
        if self.search_focus {
            self.on_search_key(key);
        } else if self.person_view.is_some() {
            self.on_person_key(key);
        } else {
            self.on_nav_key(key);
        }
    }

    fn on_person_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.running = false,
            KeyCode::Esc | KeyCode::Backspace => self.close_person(),
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(view) = &mut self.person_view {
                    view.scroll = view.scroll.saturating_add(1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(view) = &mut self.person_view {
                    view.scroll = view.scroll.saturating_sub(1);
                }
            }
            KeyCode::Char('g') => {
                if let Some(view) = &mut self.person_view {
                    view.scroll = 0;
                }
            }
            KeyCode::Char('G') => {
                if let Some(view) = &mut self.person_view {
                    view.scroll = u16::MAX;
                }
            }
            KeyCode::Char('n') => self.move_person_cursor(1),
            KeyCode::Char('p') => self.move_person_cursor(-1),
            KeyCode::Enter => self.activate_person_link(),
            _ => {}
        }
    }

    /// Collaborations of the open person page, in render order.
    fn person_links(&self) -> Vec<PageLink> {
        let Some(view) = &self.person_view else {
            return Vec::new();
        };
        let Some(data) = &view.data else {
            return Vec::new();
        };
        data.sections
            .iter()
            .flat_map(|section| section.works.iter())
            .map(|work| PageLink::Media(Box::new(work.media.clone())))
            .collect()
    }

    fn move_person_cursor(&mut self, delta: i64) {
        let count = self.person_links().len();
        if count == 0 {
            return;
        }
        let Some(view) = &mut self.person_view else {
            return;
        };
        view.cursor = match view.cursor {
            None if delta > 0 => Some(0),
            None => Some(count - 1),
            Some(index) => {
                let next = index as i64 + delta;
                if next < 0 {
                    None
                } else {
                    Some((next as usize).min(count - 1))
                }
            }
        };
        view.reveal = true;
        self.dirty = true;
    }

    fn activate_person_link(&mut self) {
        let Some(view) = &self.person_view else {
            return;
        };
        if !view.cursor_visible {
            return;
        }
        let link = view
            .cursor
            .and_then(|index| self.person_links().get(index).cloned());
        if let Some(link) = link {
            self.follow(link);
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.search_focus = false,
            KeyCode::Enter => {
                self.search_focus = false;
                self.search_due = None;
                self.apply_search();
            }
            KeyCode::Backspace => {
                self.search_input.pop();
                self.schedule_search();
            }
            KeyCode::Char(c) => {
                self.search_input.push(c);
                self.schedule_search();
            }
            _ => {}
        }
    }

    fn on_nav_key(&mut self, key: KeyEvent) {
        if self.detail {
            self.on_page_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.running = false,
            KeyCode::Esc => self.detail = false,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = self.items.len().saturating_sub(1),
            KeyCode::Enter => self.open_page(),
            KeyCode::Char('/') => {
                self.search_input.clear();
                self.search_focus = true;
            }
            KeyCode::Tab => self.toggle_source(),
            KeyCode::Char('r') => self.refetch(),
            KeyCode::Char('m') | KeyCode::Char('M') => self.open_mine(),
            KeyCode::Backspace => self.back_from_browse(),
            _ => {}
        }
    }

    /// The media page has two independent columns. `Tab` moves the focus, the
    /// vertical keys and the wheel scroll only the focused one. Left/right are
    /// ignored and never change the selected media.
    fn on_page_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.running = false,
            KeyCode::Esc | KeyCode::Backspace => self.back(),
            KeyCode::Tab => {
                self.page_focus = match self.page_focus {
                    PagePane::Sidebar => PagePane::Content,
                    PagePane::Content => PagePane::Sidebar,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll_focused(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_focused(-1),
            KeyCode::Char('g') => self.scroll_focused(i64::from(i16::MIN)),
            KeyCode::Char('G') => self.scroll_focused(i64::from(i16::MAX)),
            KeyCode::Char('s') => {
                self.show_spoiler_tags = !self.show_spoiler_tags;
            }
            KeyCode::Char('n') => self.move_link(1),
            KeyCode::Char('p') => self.move_link(-1),
            KeyCode::Enter => self.activate_link(),
            KeyCode::Char('h') | KeyCode::Left => self.cycle_tab(-1),
            KeyCode::Char('l') | KeyCode::Right => self.cycle_tab(1),
            KeyCode::Char('1') => self.switch_tab(PageTab::Overview),
            KeyCode::Char('2') => self.switch_tab(PageTab::Characters),
            KeyCode::Char('3') => self.switch_tab(PageTab::Staff),
            _ => {}
        }
    }

    /// Open a page from the list: a fresh session, no history behind it.
    fn open_page(&mut self) {
        if let Some(media) = self.selected_item().cloned() {
            self.page_history.clear();
            self.browse_origin = None;
            self.load_page(media);
        }
    }

    /// Follow an entry on the page itself: remember where we came from so
    /// `Backspace` can walk back through the history.
    fn open_media(&mut self, media: Media) {
        if let Some(current) = self.page_media() {
            if current.id == media.id {
                return;
            }
            self.page_history.push(current);
        }
        self.load_page(media);
    }

    /// Walk back one page. With an empty history the page closes and the list
    /// comes back.
    fn back(&mut self) {
        match self.page_history.pop() {
            Some(media) => self.load_page(media),
            None => self.close_page(),
        }
    }

    /// The media page is its own scope: loading it starts a detail request and
    /// cancels whatever the previous page had in flight.
    fn load_page(&mut self, media: Media) {
        self.close_detail_scope();
        // The page owns the keyboard from here on.
        self.search_focus = false;
        self.detail = true;
        self.page_focus = PagePane::Content;
        self.sidebar_scroll = 0;
        self.content_scroll = 0;
        self.link_cursor = None;
        self.tag_cursor = None;
        self.cursor_visible = true;
        self.reveal_cursor = false;
        self.tab = PageTab::Overview;
        self.characters_tab = CreditList::default();
        self.staff_tab = CreditList::default();
        self.detail_loading = true;
        self.page_subject = Some(media.clone());

        let scope = self.net.mount();
        self.detail_scope = Some(scope);
        self.net.request(
            scope,
            ReqKey::Detail(media.kind, media.id),
            Job::Detail {
                id: media.id,
                kind: media.kind,
            },
        );
    }

    fn close_detail_scope(&mut self) {
        self.detail_data = None;
        self.detail_loading = false;
        self.detail_error = None;
        self.page_links.clear();
        if let Some(scope) = self.detail_scope.take() {
            self.net.retire(scope);
        }
    }

    fn close_page(&mut self) {
        self.detail = false;
        self.page_focus = PagePane::Content;
        self.sidebar_scroll = 0;
        self.content_scroll = 0;
        self.link_cursor = None;
        self.tag_cursor = None;
        self.cursor_visible = true;
        self.reveal_cursor = false;
        self.tab = PageTab::Overview;
        self.characters_tab = CreditList::default();
        self.staff_tab = CreditList::default();
        self.page_subject = None;
        self.page_history.clear();
        self.close_detail_scope();
    }

    fn move_selection(&mut self, delta: i64) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        let next = self.selected as i64 + delta;
        self.selected = next.clamp(0, last as i64) as usize;
        self.maybe_prefetch();
    }

    /// Infinite scroll: when the selection nears the end of the loaded items,
    /// ask for the next page in the same scope. Nothing is cancelled here: the
    /// rows already on screen stay valid.
    fn maybe_prefetch(&mut self) {
        if !self.has_next || self.list_loading || self.items.is_empty() {
            return;
        }
        if self.selected + PREFETCH_AHEAD < self.items.len() {
            return;
        }
        if matches!(self.source, Source::Mine) {
            self.prefetch_mine();
            return;
        }
        self.page += 1;
        self.request_page();
    }

    fn toggle_source(&mut self) {
        let query = self.search_input.trim().to_string();
        self.source = match &self.source {
            Source::Trending if !query.is_empty() => Source::Search(query),
            Source::Trending => return,
            _ => Source::Trending,
        };
        self.browse_origin = None;
        self.page = 1;
        self.refetch();
    }

    fn open_mine(&mut self) {
        if self.viewer.is_none() {
            // Anonymous sessions have no personal lists: stay on trending.
            if !matches!(self.source, Source::Trending) {
                self.source = Source::Trending;
                self.page = 1;
                self.refetch();
            }
            return;
        }
        if matches!(self.source, Source::Mine) {
            return;
        }
        self.source = Source::Mine;
        self.browse_origin = None;
        self.page = 1;
        self.refetch();
    }

    fn request_mine(&mut self, kind: MediaKind, page: u32) {
        let Some(user_id) = self.viewer else {
            return;
        };
        if self.mine.section(kind).loading {
            return;
        }
        let scope = self.scope;
        self.mine.section_mut(kind).loading = true;
        self.list_loading = true;
        self.dirty = true;
        self.net.request(
            scope,
            ReqKey::Mine(kind, page),
            Job::Mine {
                kind,
                user_id,
                page,
                per_page: PER_PAGE,
            },
        );
    }

    fn prefetch_mine(&mut self) {
        let mut next = Vec::new();
        for kind in [MediaKind::Anime, MediaKind::Manga] {
            let section = self.mine.section(kind);
            if section.has_next && !section.loading {
                next.push((kind, section.page + 1));
            }
        }
        for (kind, page) in next {
            self.request_mine(kind, page);
        }
    }

    fn rebuild_mine(&mut self) {
        let selected = self.items.get(self.selected).map(|media| media.id);
        let anime = self.mine.anime.items.clone();
        let manga = self.mine.manga.items.clone();
        // Before both first pages land, one section can arrive alone; that must
        // not move the focus to the first manga row.
        let ready = self.mine.anime.page > 0 && self.mine.manga.page > 0;
        let keep_selection = self.mine_ready && ready;

        self.items.clear();
        self.my_progress.clear();
        self.list_headers.clear();
        if !anime.is_empty() {
            self.list_headers.push((0, "ANIME IN PROGRESS".into()));
        }
        for entry in anime {
            self.my_progress.insert(entry.media.id, entry.progress);
            self.items.push(entry.media);
        }
        if !manga.is_empty() {
            self.list_headers
                .push((self.items.len(), "MANGA IN PROGRESS".into()));
        }
        for entry in manga {
            self.my_progress.insert(entry.media.id, entry.progress);
            self.items.push(entry.media);
        }
        self.has_next = self.mine.has_next();
        self.selected = if keep_selection {
            selected
                .and_then(|id| self.items.iter().position(|media| media.id == id))
                .unwrap_or(0)
        } else {
            0
        };
        self.mine_ready = ready;
    }

    fn schedule_search(&mut self) {
        self.search_due = Some(Instant::now() + SEARCH_DEBOUNCE);
    }

    fn apply_search(&mut self) {
        let query = self.search_input.trim().to_string();
        let next = if query.is_empty() {
            Source::Trending
        } else {
            Source::Search(query)
        };
        if next == self.source {
            return;
        }
        self.browse_origin = None;
        self.source = next;
        self.page = 1;
        self.refetch();
    }

    /// Retire the current widget scope, mount a fresh one, and load its first
    /// page. Everything still in flight for the old view dies here.
    fn refetch(&mut self) {
        self.close_page();
        self.net.retire(self.scope);
        self.scope = self.net.mount();
        self.pending_covers.clear();
        self.failed_covers.clear();
        self.items.clear();
        self.my_progress.clear();
        self.mine = MineLists::default();
        self.mine_ready = false;
        self.list_headers.clear();
        self.selected = 0;
        self.page = 1;
        self.error = None;
        self.sidebar_scroll = 0;
        self.content_scroll = 0;
        self.list_targets.clear();
        self.dirty = true;
        self.request_page();
    }

    fn request_page(&mut self) {
        self.list_loading = true;
        let (scope, page) = (self.scope, self.page);
        match &self.source {
            Source::Trending => {
                self.net.request(
                    scope,
                    ReqKey::Trending(page),
                    Job::Trending {
                        page,
                        per_page: PER_PAGE,
                    },
                );
            }
            Source::Search(query) => {
                self.net.request(
                    scope,
                    ReqKey::Search(query.clone(), page),
                    Job::Search {
                        query: query.clone(),
                        page,
                        per_page: PER_PAGE,
                    },
                );
            }
            Source::Genre(genre) => {
                self.net.request(
                    scope,
                    ReqKey::Browse(format!("genre:{genre}"), page),
                    Job::Browse {
                        genre: Some(genre.clone()),
                        tag: None,
                        page,
                        per_page: PER_PAGE,
                    },
                );
            }
            Source::Tag(tag) => {
                self.net.request(
                    scope,
                    ReqKey::Browse(format!("tag:{tag}"), page),
                    Job::Browse {
                        genre: None,
                        tag: Some(tag.clone()),
                        page,
                        per_page: PER_PAGE,
                    },
                );
            }
            Source::Mine => {
                self.request_mine(MediaKind::Anime, 1);
                self.request_mine(MediaKind::Manga, 1);
            }
        }
    }

    pub fn on_net(&mut self, msg: NetMsg) {
        match msg {
            NetMsg::List {
                scope,
                req,
                page,
                items,
                has_next,
            } => {
                if scope != self.scope || !self.net.is_current(scope, req) {
                    return;
                }
                self.net.done(scope, req);
                self.list_loading = false;
                self.has_next = has_next;
                if page <= 1 {
                    self.items = items;
                } else {
                    self.items.extend(items);
                }
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
                self.error = None;
                self.dirty = true;
            }
            NetMsg::Mine {
                scope,
                req,
                kind,
                page,
                items,
                has_next,
            } => {
                if scope != self.scope || !self.net.is_current(scope, req) {
                    return;
                }
                self.net.done(scope, req);
                let section = self.mine.section_mut(kind);
                section.loading = false;
                section.has_next = has_next;
                section.page = page;
                if page <= 1 {
                    section.items = items;
                } else {
                    section.items.extend(items);
                }
                self.list_loading = self.mine.loading();
                self.rebuild_mine();
                self.error = None;
                self.dirty = true;
            }
            NetMsg::Cover {
                scope,
                req,
                url,
                w,
                h,
                proto,
            } => {
                self.net.done(scope, req);
                // Content addressed: useful even if the requesting scope is gone.
                self.covers.insert((url.clone(), w, h), proto);
                self.pending_covers.remove(&(url, w, h));
                self.dirty = true;
            }
            NetMsg::Detail { scope, req, data } => {
                if Some(scope) != self.detail_scope || !self.net.is_current(scope, req) {
                    self.net.done(scope, req);
                    return;
                }
                self.net.done(scope, req);
                self.detail_loading = false;
                self.detail_error = None;
                self.detail_data = Some(*data);
                if let Some(cursor) = self.link_cursor
                    && cursor >= self.content_links().len()
                {
                    self.link_cursor = None;
                }
                self.dirty = true;
            }
            NetMsg::Person {
                scope,
                req,
                page,
                data,
            } => {
                if Some(scope) != self.person_scope || !self.net.is_current(scope, req) {
                    self.net.done(scope, req);
                    return;
                }
                self.net.done(scope, req);
                let Some(view) = &mut self.person_view else {
                    return;
                };
                view.loading = false;
                view.error = None;
                view.page = page;
                match &mut view.data {
                    None => view.data = Some(*data),
                    Some(existing) if page <= 1 => *existing = *data,
                    Some(existing) => {
                        for section in data.sections {
                            match existing
                                .sections
                                .iter_mut()
                                .find(|current| current.title == section.title)
                            {
                                Some(current) => current.works.extend(section.works),
                                None => existing.sections.push(section),
                            }
                        }
                        existing.has_next = data.has_next;
                    }
                }
                self.dirty = true;
            }
            NetMsg::Credits {
                scope,
                req,
                kind,
                page,
                items,
                has_next,
            } => {
                if Some(scope) != self.detail_scope || !self.net.is_current(scope, req) {
                    self.net.done(scope, req);
                    return;
                }
                self.net.done(scope, req);
                let list = self.credit_list(kind);
                list.loading = false;
                list.has_next = has_next;
                list.page = page;
                if page <= 1 {
                    list.items = items;
                } else {
                    list.items.extend(items);
                }
                self.dirty = true;
            }
            NetMsg::Failed {
                scope,
                req,
                key,
                err,
                ..
            } => {
                self.net.done(scope, req);
                // A failure from a superseded request is not user visible.
                let known = scope == self.scope || Some(scope) == self.detail_scope;
                if !known || !self.net.is_current(scope, req) {
                    return;
                }
                match key {
                    ReqKey::Cover { url, w, h } => {
                        self.pending_covers.remove(&(url.clone(), w, h));
                        self.failed_covers.insert((url, w, h));
                    }
                    ReqKey::Mine(kind, _) => {
                        self.mine.section_mut(kind).loading = false;
                        self.list_loading = self.mine.loading();
                        self.detail_error = None;
                        self.error = Some(err);
                    }
                    ReqKey::Detail(_, _) => {
                        self.detail_loading = false;
                        self.detail_error = Some(err);
                    }
                    ReqKey::Credits(kind, _) => {
                        self.credit_list(kind).loading = false;
                        self.detail_error = Some(err);
                    }
                    ReqKey::Person(_, _, _) => {
                        if let Some(view) = &mut self.person_view {
                            view.loading = false;
                            view.error = Some(err);
                        }
                    }
                    _ => {
                        self.list_loading = false;
                        self.error = Some(err);
                    }
                }
                self.dirty = true;
            }
        }
    }

    /// Debounce timer and spinner. Returns true when a redraw is needed.
    pub fn tick(&mut self) -> bool {
        let mut redraw = false;

        // Selection can reach the end through keys handled outside
        // `move_selection` (g/G), so check here as well.
        self.maybe_prefetch();

        if let Some(due) = self.search_due
            && Instant::now() >= due
        {
            self.search_due = None;
            self.apply_search();
            redraw = true;
        }

        if self.list_loading || !self.pending_covers.is_empty() {
            self.spinner = self.spinner.wrapping_add(1);
            redraw = true;
        }
        redraw
    }

    /// Cached cover for `url`, or `None` while it loads. Requests the cover on
    /// first sight; the request belongs to the current scope. `key` is the
    /// semantic disk-cache key, e.g. `anime:21:cover`.
    pub fn cover(&mut self, key: &str, url: &str, w: u16, h: u16) -> Option<Arc<Protocol>> {
        let cache_key = (url.to_string(), w, h);
        if let Some(proto) = self.covers.get(&cache_key) {
            return Some(proto.clone());
        }
        // No live scope means the view is being replaced: do not start work.
        if self.failed_covers.contains(&cache_key) || !self.net.is_live(self.scope) {
            return None;
        }
        if self.pending_covers.insert(cache_key.clone()) {
            let scope = self.scope;
            self.net.request(
                scope,
                ReqKey::Cover {
                    url: cache_key.0.clone(),
                    w,
                    h,
                },
                Job::Cover {
                    url: cache_key.0.clone(),
                    w,
                    h,
                    key: key.to_string(),
                },
            );
        }
        None
    }

    pub fn selected_item(&self) -> Option<&Media> {
        self.items.get(self.selected)
    }

    /// True while the page waits for relations and recommendations.
    pub fn page_loading(&self) -> bool {
        self.detail_loading
    }

    /// Media the page renders: the fetched record when available, the clicked
    /// node while loading, the list selection as last resort.
    /// Number of pages behind the current one.
    pub fn history_depth(&self) -> usize {
        self.page_history.len()
    }

    pub fn page_media(&self) -> Option<Media> {
        self.detail_data
            .as_ref()
            .map(|data| data.media.clone())
            .or_else(|| self.page_subject.clone())
            .or_else(|| self.selected_item().cloned())
    }

    /// Viewer's list entry for the open page, once the detail request landed.
    pub fn list_entry(&self) -> Option<&crate::tui::model::MediaListEntry> {
        let data = self.detail_data.as_ref()?;
        let entry = data.list_entry.as_ref()?;
        (entry.status.is_some()
            || entry.score.is_some_and(|score| score > 0.0)
            || entry.progress.is_some_and(|progress| progress > 0)
            || entry.repeat.is_some_and(|repeat| repeat > 0)
            || entry
                .notes
                .as_ref()
                .is_some_and(|notes| !notes.trim().is_empty()))
        .then_some(entry)
    }

    /// Local episode for a media, possibly ahead of AniList. The UI treats it
    /// as the truth when it is greater than the remote progress.
    pub fn local_progress(&self, id: i64) -> Option<i64> {
        self.local_progress.get(&id).copied()
    }

    /// Progress shown for a current-list row. Local episode wins for anime.
    pub fn my_progress(&self, id: i64, kind: MediaKind) -> Option<i64> {
        let remote = self.my_progress.get(&id).copied()?;
        if kind != MediaKind::Anime {
            return Some(remote);
        }
        Some(self.local_progress(id).unwrap_or(remote).max(remote))
    }

    /// Section headers for the current list: `(item index, label)`.
    pub fn list_headers(&self) -> &[(usize, String)] {
        &self.list_headers
    }

    #[cfg(test)]
    pub fn scope_id_for_test(&self) -> ScopeId {
        self.scope
    }

    #[cfg(test)]
    pub fn set_items_for_test(&mut self, items: Vec<Media>) {
        self.items = items;
    }

    #[cfg(test)]
    pub fn set_cover_for_test(&mut self, url: &str, w: u16, h: u16, proto: Arc<Protocol>) {
        self.covers.insert((url.to_string(), w, h), proto);
    }

    #[cfg(test)]
    pub fn set_detail_for_test(&mut self, detail: MediaDetail) {
        self.detail_data = Some(detail);
        self.detail_loading = false;
    }

    #[cfg(test)]
    pub fn detail_scope_for_test(&self) -> Option<ScopeId> {
        self.detail_scope
    }

    #[cfg(test)]
    pub fn page_targets_for_test(&self) -> Vec<(Rect, i64)> {
        self.page_links
            .iter()
            .filter_map(|(rect, link)| match link {
                PageLink::Media(media) => Some((*rect, media.id)),
                _ => None,
            })
            .collect()
    }

    #[cfg(test)]
    pub fn page_links_for_test(&self) -> Vec<(Rect, PageLink)> {
        self.page_links.clone()
    }

    #[cfg(test)]
    pub fn tab_targets_for_test(&self) -> Vec<(Rect, PageTab)> {
        self.tab_targets.clone()
    }

    #[cfg(test)]
    pub fn list_targets_for_test(&self) -> Vec<(Rect, i64)> {
        self.list_targets.clone()
    }

    #[cfg(test)]
    pub fn net_req_for_test(&self) -> Option<u64> {
        self.net.last_req_for_test(self.scope)
    }

    #[cfg(test)]
    pub fn person_scope_for_test(&self) -> Option<ScopeId> {
        self.person_scope
    }

    #[cfg(test)]
    pub fn set_person_for_test(&mut self, view: PersonView) {
        self.person_view = Some(view);
    }

    #[cfg(test)]
    pub fn detail_req_for_test(&self) -> Option<u64> {
        self.detail_scope
            .and_then(|scope| self.net.last_req_for_test(scope))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc;

    use super::{App, Source};
    use crate::tui::model::MediaKind;

    fn app() -> App {
        let (tx, _rx) = mpsc::channel(8);
        App::new(Arc::new(Picker::halfblocks()), tx)
    }

    fn media(id: i64) -> crate::tui::model::Media {
        crate::tui::model::Media {
            id,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn anonymous_sessions_stay_on_trending() {
        let mut app = app();
        assert_eq!(app.source, Source::Trending);

        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.on_key(key(KeyCode::Char('m')));
        assert_eq!(app.source, Source::Trending);
        app.on_key(key(KeyCode::Char('M')));
        assert_eq!(app.source, Source::Trending);

        app.source = Source::Genre("Action".into());
        app.on_key(key(KeyCode::Char('m')));
        assert_eq!(app.source, Source::Trending);
    }

    #[tokio::test]
    async fn logged_in_sessions_default_to_my_lists() {
        let (tx, _rx) = mpsc::channel(8);
        let mut app = App::with_account(
            Arc::new(Picker::halfblocks()),
            tx,
            None,
            None,
            Some(7),
            std::collections::HashMap::new(),
        );
        assert_eq!(app.source, Source::Mine);

        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.on_key(key(KeyCode::Char('m')));
        assert_eq!(app.source, Source::Mine);
        app.on_key(key(KeyCode::Char('M')));
        assert_eq!(app.source, Source::Mine);
    }

    #[tokio::test]
    async fn my_lists_combine_anime_and_manga_with_headers() {
        use crate::tui::model::MyListEntry;

        let mut app = app();
        let mut manga = media(2);
        manga.kind = MediaKind::Manga;
        app.mine.anime.items = vec![MyListEntry {
            media: media(1),
            progress: 3,
        }];
        app.mine.manga.items = vec![MyListEntry {
            media: manga,
            progress: 5,
        }];

        app.rebuild_mine();

        assert_eq!(app.items.len(), 2);
        assert_eq!(
            app.list_headers(),
            &[
                (0, "ANIME IN PROGRESS".to_string()),
                (1, "MANGA IN PROGRESS".to_string()),
            ]
        );
        assert_eq!(app.my_progress(1, MediaKind::Anime), Some(3));
        assert_eq!(app.my_progress(2, MediaKind::Manga), Some(5));
    }

    #[tokio::test]
    async fn manga_arriving_first_does_not_take_focus() {
        use crate::tui::model::MyListEntry;

        let mut app = app();
        let mut manga = media(2);
        manga.kind = MediaKind::Manga;
        app.mine.manga.items = vec![MyListEntry {
            media: manga,
            progress: 1,
        }];
        app.mine.manga.page = 1;
        app.rebuild_mine();

        app.mine.anime.items = vec![MyListEntry {
            media: media(1),
            progress: 1,
        }];
        app.mine.anime.page = 1;
        app.rebuild_mine();

        assert_eq!(app.selected, 0);
        assert_eq!(app.items[0].id, 1, "focus starts on the first anime");
    }

    #[tokio::test]
    async fn selection_is_clamped_to_the_loaded_page() {
        let mut app = app();
        app.items = vec![media(1), media(2)];

        app.move_selection(-5);
        assert_eq!(app.selected, 0);

        app.move_selection(99);
        assert_eq!(app.selected, 1);
    }

    #[tokio::test]
    async fn reaching_the_end_prefetches_the_next_page_in_the_same_scope() {
        let mut app = app();
        app.set_items_for_test((1..=30).map(media).collect());
        app.list_loading = false;
        app.has_next = true;
        app.page = 1;
        let scope = app.scope_id_for_test();

        app.selected = 10;
        app.move_selection(1);
        assert_eq!(app.page, 1, "far from the end: no request");

        app.selected = 29;
        app.move_selection(1);
        assert_eq!(app.page, 2);
        assert_eq!(
            app.scope_id_for_test(),
            scope,
            "same scope, nothing cancelled"
        );
        assert!(app.list_loading);
    }

    #[tokio::test]
    async fn page_after_the_first_is_appended() {
        let mut app = app();
        app.set_items_for_test(vec![media(1)]);
        app.list_loading = true;
        app.page = 2;
        let scope = app.scope_id_for_test();
        let req = app.net_req_for_test().expect("list request registered");

        app.on_net(crate::tui::net::NetMsg::List {
            scope,
            req,
            page: 2,
            items: vec![media(2), media(3)],
            has_next: false,
        });

        let ids: Vec<i64> = app.items.iter().map(|media| media.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert!(!app.has_next);
        assert!(!app.list_loading);

        app.wheel(1);
        assert_eq!(app.page, 2, "no further page when has_next is false");
    }

    #[tokio::test]
    async fn page_keys_scroll_the_focused_pane_only() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::tui::app::PagePane;

        let mut app = app();
        app.set_items_for_test(vec![media(1), media(2)]);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        app.on_key(key(KeyCode::Enter));
        assert!(app.detail);
        assert_eq!(app.page_focus, PagePane::Content, "content is the default");

        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.content_scroll, 2);
        assert_eq!(app.sidebar_scroll, 0);

        app.on_key(key(KeyCode::Up));
        assert_eq!(app.content_scroll, 1);

        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.page_focus, PagePane::Sidebar);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.sidebar_scroll, 1);
        assert_eq!(app.content_scroll, 1, "content does not move");

        app.on_key(key(KeyCode::Char('g')));
        assert_eq!(app.sidebar_scroll, 0);
        assert_eq!(app.content_scroll, 1);

        // h/l (and arrows) switch tabs; they never change the selected media.
        app.on_key(key(KeyCode::Char('l')));
        assert_eq!(app.tab, crate::tui::app::PageTab::Characters);
        assert_eq!(app.selected, 0);
        app.on_key(key(KeyCode::Right));
        assert_eq!(app.tab, crate::tui::app::PageTab::Staff);
        app.on_key(key(KeyCode::Left));
        assert_eq!(app.tab, crate::tui::app::PageTab::Characters);
        app.on_key(key(KeyCode::Char('1')));
        assert_eq!(app.tab, crate::tui::app::PageTab::Overview);

        // Enter does nothing on the page: no navigation, no scroll.
        app.on_key(key(KeyCode::Enter));
        assert!(app.detail);
        assert_eq!(app.sidebar_scroll, 0);

        app.on_key(key(KeyCode::Backspace));
        assert!(!app.detail);
        assert_eq!(app.content_scroll, 0, "scroll resets when the page closes");

        app.on_key(key(KeyCode::Enter));
        assert!(app.detail);
        app.on_key(key(KeyCode::Esc));
        assert!(!app.detail);
    }

    #[tokio::test]
    async fn wheel_scrolls_the_list_and_the_focused_page_pane() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        use crate::tui::app::PagePane;

        let mut app = app();
        app.set_items_for_test((1..=10).map(media).collect());
        let wheel = |kind| MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(app.selected, 3);

        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.selected, 0, "cannot scroll above the first row");

        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.selected, 0);

        app.detail = true;
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(app.content_scroll, 3);
        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.content_scroll, 0);

        app.page_focus = PagePane::Sidebar;
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(app.sidebar_scroll, 3);
        assert_eq!(app.content_scroll, 0);
    }

    #[tokio::test]
    async fn keyboard_walks_the_page_links_and_enter_opens_them() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::tui::app::PagePane;
        use crate::tui::model::{MediaDetail, Relation};

        let mut app = app();
        let main = media(1);
        app.set_items_for_test(vec![main.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: main,
            relations: vec![Relation {
                relation_type: "SEQUEL".into(),
                media: media(2),
            }],
            recommendations: vec![media(3)],
            ..Default::default()
        });
        app.detail = true;
        app.page_focus = PagePane::Content;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.link_cursor, Some(0));

        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.link_cursor, Some(1));
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.link_cursor, Some(1), "stops at the last link");

        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(app.link_cursor, Some(0));
        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(app.link_cursor, None, "going back past the first deselects");

        app.on_key(key(KeyCode::Char('n')));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.page_media().map(|media| media.id), Some(2));
        assert_eq!(app.history_depth(), 1);
        assert_eq!(app.link_cursor, None, "a new page starts without a cursor");
    }

    #[tokio::test]
    async fn browsing_a_genre_can_be_undone_with_backspace() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::tui::app::PagePane;
        use crate::tui::model::{MediaDetail, Relation};

        let mut app = app();
        let first = media(1);
        let second = crate::tui::model::Media {
            genres: vec!["Action".into()],
            ..media(2)
        };
        app.set_items_for_test(vec![first.clone()]);
        app.set_detail_for_test(MediaDetail {
            media: first,
            relations: vec![Relation {
                relation_type: "SEQUEL".into(),
                media: second.clone(),
            }],
            recommendations: Vec::new(),
            ..Default::default()
        });
        app.detail = true;
        app.page_focus = PagePane::Content;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        // Page 1 -> follow the relation: page 2, with page 1 in history.
        app.on_key(key(KeyCode::Char('n')));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.page_media().map(|media| media.id), Some(2));
        assert_eq!(app.history_depth(), 1);

        // Page 2 has a genre: focus it and browse it.
        app.on_key(key(KeyCode::Char('n')));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.source, Source::Genre("Action".into()));
        assert!(!app.detail);
        assert!(app.has_browse_origin());

        // Back returns to page 2 with the old history intact.
        app.on_key(key(KeyCode::Backspace));
        assert!(app.detail);
        assert_eq!(app.page_media().map(|media| media.id), Some(2));
        assert_eq!(app.history_depth(), 1);
        assert!(!app.has_browse_origin());

        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.page_media().map(|media| media.id), Some(1));
        assert_eq!(app.history_depth(), 0);

        app.on_key(key(KeyCode::Backspace));
        assert!(!app.detail);
    }

    #[tokio::test]
    async fn character_and_staff_tabs_fetch_pages() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::tui::app::{CreditList, PageTab};
        use crate::tui::model::{Credit, MediaDetail};

        let mut app = app();
        let main = media(1);
        app.set_items_for_test(vec![main.clone()]);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.set_detail_for_test(MediaDetail {
            media: main,
            characters: vec![Credit::default()],
            staff: vec![Credit::default()],
            ..Default::default()
        });

        app.on_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(app.tab, PageTab::Characters);
        assert!(app.characters_tab.loading, "page 1 requested");

        let scope = app.detail_scope_for_test().expect("detail scope");
        let req = app.detail_req_for_test().expect("credit request");
        app.on_net(crate::tui::net::NetMsg::Credits {
            scope,
            req,
            kind: crate::tui::model::CreditKind::Characters,
            page: 1,
            items: vec![Credit {
                name: "D".into(),
                ..Default::default()
            }],
            has_next: true,
        });
        let list: &CreditList = &app.characters_tab;
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].name, "D");
        assert!(list.has_next);
        assert!(!list.loading);

        app.on_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(app.tab, PageTab::Staff);
        assert!(app.staff_tab.loading, "staff page 1 requested");

        app.on_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        assert_eq!(app.tab, PageTab::Overview);
        app.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(app.tab, PageTab::Characters);
    }

    #[tokio::test]
    async fn staff_and_character_rows_open_their_person_page() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::tui::app::PagePane;
        use crate::tui::model::{Credit, MediaDetail, PersonKind, PersonPageData};

        let mut app = app();
        app.set_items_for_test(vec![media(1)]);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.set_detail_for_test(MediaDetail {
            media: media(1),
            characters: vec![Credit {
                id: Some(555),
                name: "D".into(),
                ..Default::default()
            }],
            staff: vec![Credit {
                id: Some(97349),
                name: "Hiroyuki Imaishi".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        app.page_focus = PagePane::Content;

        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.on_key(key(KeyCode::Char('n')));
        app.on_key(key(KeyCode::Enter));
        assert!(app.person_open(), "character row opens its page");
        assert_eq!(
            app.person_view
                .as_ref()
                .and_then(|view| view.data.as_ref())
                .map(|data| data.kind),
            None,
            "data still loading"
        );

        let scope = app.person_scope_for_test().expect("person scope");
        let req = app
            .net
            .last_req_for_test(scope)
            .expect("person request registered");
        app.on_net(crate::tui::net::NetMsg::Person {
            scope,
            req,
            page: 1,
            data: Box::new(PersonPageData {
                kind: PersonKind::Character,
                id: 555,
                name: "D".into(),
                ..Default::default()
            }),
        });
        assert!(app.person_view.as_ref().unwrap().data.is_some());

        app.on_key(key(KeyCode::Backspace));
        assert!(!app.person_open(), "back closes the person page");
        assert!(app.detail, "the media page is still open underneath");
        assert!(app.person_scope_for_test().is_none(), "scope retired");

        // Next link is the staff member.
        app.on_key(key(KeyCode::Char('n')));
        app.on_key(key(KeyCode::Enter));
        assert!(app.person_open());
        let scope = app.person_scope_for_test().expect("staff scope");
        let req = app.net.last_req_for_test(scope).expect("request");
        app.on_net(crate::tui::net::NetMsg::Person {
            scope,
            req,
            page: 1,
            data: Box::new(PersonPageData {
                kind: PersonKind::Staff,
                id: 97349,
                name: "Hiroyuki Imaishi".into(),
                ..Default::default()
            }),
        });
        assert_eq!(
            app.person_view
                .as_ref()
                .and_then(|view| view.data.as_ref())
                .map(|data| data.kind),
            Some(PersonKind::Staff)
        );
    }

    #[tokio::test]
    async fn the_page_owns_a_cancellable_detail_scope() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = app();
        app.set_items_for_test(vec![media(7)]);
        assert!(app.detail_scope_for_test().is_none());

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.detail);
        assert_eq!(app.history_depth(), 0, "opening from the list is fresh");
        assert!(app.detail_scope_for_test().is_some());
        assert!(app.page_loading());

        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(!app.detail);
        assert!(app.detail_scope_for_test().is_none());
        assert!(!app.page_loading());
    }

    #[tokio::test]
    async fn search_source_switches_only_when_the_query_is_committed() {
        let mut app = app();
        app.has_next = true;

        app.search_input = "frieren".into();
        app.apply_search();

        assert_eq!(app.source, Source::Search("frieren".into()));

        app.search_input.clear();
        app.apply_search();
        assert_eq!(app.source, Source::Trending);
    }
}
