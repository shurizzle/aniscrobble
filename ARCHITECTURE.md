# Architecture

One binary, two front ends over the same AniList account:

- **CLI** (always built): `login`, `sync`, `scrobble`. Tracks episodes locally
  and pushes progress to AniList.
- **TUI** (`tui` feature, on by default, `tui` subcommand): read-only AniList
  catalog browser. The subcommand is also the default when no subcommand is
  given. `--no-default-features` builds the CLI alone.

The CLI is blocking and single-threaded. The TUI is async and input-first. They
share the crate and the stored login: the TUI reads the token once, read-only.

## CLI

### Commands

| Command | Effect |
| --- | --- |
| `login [--force]` | Open the AniList token page, validate the pasted token with `Viewer { id }`, store `User { token, id }` |
| `scrobble [--background] [--local-only] <anilist_id> <episode>` | Record the episode locally; then sync now, sync in a detached child, or skip the network |
| `sync` | Push every pending entry to AniList |

### Storage

LMDB through `heed`, in the platform cache dir
(`directories::ProjectDirs` → `dev/shurizzle/aniscrobble/data.db`). Two
databases:

- `main` (`Str` → bincode `Bytes`): `version`, `login`, `pending`.
- `data` (`U64` → `U64`): `anilist_id` → highest episode seen locally.
- `cache` (`Str` → raw `Bytes`): TUI fetch cache, see [Disk cache](#disk-cache).
  Opened lazily and one of the three named databases, so the environment maps
  1 GiB.

`pending` is a sorted `Vec<u64>` of ids whose local progress is ahead of the
server. An entry leaves `pending` only after a successful remote update, which
makes an interrupted sync resumable. The local episode is written before any
network call, so `--local-only` is just "stop here".

### Sync

For each pending id: `Media.episodes` (public) plus `MediaList.progress`
(authenticated, 404 means "not on the list yet", treated as 0). If the local
episode is ahead, `SaveMediaListEntry` sets progress and `status` becomes
`COMPLETED` when progress reaches the episode count, `CURRENT` otherwise. The
local value is then updated and the id removed from `pending`. One bad entry is
reported and skipped; only database errors are fatal.

Network I/O is blocking `ureq` with one shared agent. `--background` detaches:
double `fork` + `setsid` + stdio on `/dev/null` on unix, a `sync` child with
`CREATE_NO_WINDOW | DETACHED_PROCESS` on Windows.

## TUI

### Config

`src/config.rs` reads `config.toml` from `dirs::config_dir()` /
`aniscrobble/config.toml`. A missing file is not an error (defaults apply); an
invalid file is reported once in the footer and the defaults apply. Only one key
exists today:

```toml
[external]
command = "/path/to/bin"
```

The value is a plain path, not a shell string: the binary is run directly with
two arguments, `anime|manga` and the id. Nothing is parsed or interpolated.

### Goal

Responsiveness first. User input has absolute priority over network work. A
request that is no longer useful is cancelled, not awaited and discarded later
only.

Non-goals for the TUI: login UI, list mutations, persistence of the catalog,
background sync. The TUI reads the stored login once, read-only.

### Account data

At startup the TUI opens the CLI's database and reads the stored login and the
local episode for every media, if any. The token never leaves the process and is
attached only to the GraphQL client: covers use a second, token-free
`reqwest::Client` so the CDN never sees it. Failure to open the database or to
find a login is not an error; the TUI runs as an anonymous viewer.

The media page detail query asks for `mediaListEntry { status score progress
repeat notes }`. When AniList returns an entry and at least one field is
meaningful, the sidebar shows it right under the cover: `My Status`, `My Score`,
`Progress`, `Rewatched`, `Notes`. `Progress` is `max(local episode, remote
progress)`: a pending local scrobble is the truth even when AniList has not been
synced yet. The value is `progress / episodes` when the episode count is known.
Without any list entry, a local episode still produces a `Progress` row. Logged
out with no local episode, no rows are added. The detail response is dropped
when the page closes; only the disk cache keeps a copy. The TUI never writes the
database.

### My lists

A logged-in session starts on `Source::Mine`; an anonymous one starts on
`Source::Trending`. `m` / `M` (or `Tab` back) switch between the current lists
and trending. The source is a normal list scope with infinite scroll, so
selection, prefetch, click targets and page opening are shared with the catalog
list. Without a stored login `m`/`M` are ignored and the source stays trending.

The page is assembled from two sections, anime first and manga second, each with
its own page cursor:

- `MineLists { anime: MineSection, manga: MineSection }`, where a section holds
  `items: Vec<MyListEntry>`, `page`, `has_next`, `loading`.
- `request_page` issues both page-1 requests. Prefetch near the end issues the
  next page of every section that still has one.
- Every response updates only its own section, then `rebuild_mine` concatenates
  anime then manga into `items`, rebuilds `my_progress`, and records
  `list_headers` (`0` → `ANIME IN PROGRESS`, anime length → `MANGA IN PROGRESS`)
  for the list renderer. The first rebuild focuses index 0, so the first anime
  wins even when the manga page answers first; after both first pages have
  landed, the selected media id is preserved when an earlier section grows.

Each section queries the authenticated `Page.mediaList` connection:

```graphql
mediaList(userId: var, type: ANIME|MANGA, status: CURRENT, sort: UPDATED_TIME_DESC) {
  progress
  media { ...MEDIA_FIELDS }
}
```

Rows map to `MyListEntry { media, progress }`. The list shows
`format · progress / total`, where total is `episodes` for anime or `chapters`
for manga. An entry is "new" when more episodes/chapters exist than watched:
anime use `nextAiringEpisode.episode - 1` while airing, or the total when
finished; manga use `chapters` when finished. New rows render `+N new` in
yellow. For anime, local progress from `scrobble` is merged as the maximum, so
a pending sync never looks unwatched. Manga have no local progress source.

Media now carry their AniList `type`. Detail requests and cache keys are
type-aware (`anime:<id>` / `manga:<id>`), the credit tabs pass the same type,
and the sidebar shows chapters/volumes instead of episodes for manga. This also
makes a relation or recommendation opened from a manga page query manga again.

### External pages

`o` resolves the current context to an AniList URL and hands it to
`open::that_detached`: person page first (`staff` / `character`), then the open
media page, then the selected list row. The URL uses the media `type`, so manga
open under `/manga/`. Failures are shown in the footer. Search input keeps `o`
as text; the key does not act while the field has focus.

### External command

`e` does not spawn the child inside the key handler. `App::run_external` only
records `pending_external: (MediaKind, i64)`; the event loop sees it, returns
`Outcome::External`, and leaves the loop. `tui::run` then, in order:

1. drops the `EventStream`, whose reader thread would otherwise steal the
   child's stdin;
2. `term::suspend()` disables mouse capture, leaves raw mode and the alternate
   screen;
3. runs the configured binary through `spawn_blocking` with inherited stdio,
   `anime|manga` and the id as arguments; a non-zero status or a spawn failure
   prints the reason plus `Press ENTER to continue` and blocks on stdin, so the
   output stays readable before the screen is restored;
4. `term::resume()` re-enters raw mode and the alternate screen, installs a new
   `EventStream`, and marks the app dirty.

The app state is untouched across the suspension, so the same scroll, selection
and scope stay live. A non-zero status or spawn failure is also written to the
footer after the resume.
The target is the selected list row or the open media page; the person page
returns `None`, and a missing `[external] command` is ignored with a footer
message. Search input keeps `e` as text.

### Disk cache

`src/tui/cache.rs` persists every fetch except search in the CLI's LMDB
environment, so an interrupted connection does not erase what was already read.
The cache is the third named database (`cache`, `Str` → raw bytes) in `data.db`;
no loose files. Each value is `ct1` + the write time as a little-endian `u64`
epoch second + payload. Entries written before timestamps existed are treated as
expired but still usable as an offline fallback.

- **GraphQL**: each successful response body is stored under the key of the
  entity it describes:
  - `anime:<id>` / `manga:<id>` for the media detail (metadata, relations,
    credits, viewer entry),
  - `<anime|manga>:<id>:characters:<page>` and `...:staff:<page>` for the
    paginated tabs,
  - `my:<viewer id>:<anime|manga>:<page>` for the current lists,
  - `staff:<id>:<page>` and `character:<id>:<page>` for person pages,
  - `trending:<page>`, `genre:<name>:<page>`, `tag:<name>:<page>` for lists.

  Search is deliberately excluded: `search` always performs a live request and
  has no cached fallback, because a cached search result is a lie about the
  query.

  A fresh entry is served locally with no request. TTLs:

  | Data | TTL |
  | --- | --- |
  | Trending | 15 min |
  | Media detail | 30 min |
  | Credit tabs | 12 h |
  | Genre/tag browse | 24 h |
  | Staff/character page | 24 h |

  My lists are the one exception: `CacheMode::NetworkFirst` skips the fresh
  read, so every visit requests AniList. The response is cached and is only
  used when the request fails.

  Once expired, the request goes out. On a transport error or a non-success
  status the cached body is parsed instead, even when expired. Clients use a 5 s
  connect timeout and a 30 s total timeout, so the fallback is bounded even when
  the network black-holes. A page never fetched has no entry and behaves as
  before.
- **Covers**: raw bytes under `<anime|manga>:<id>:cover`,
  `...:cover:large` and `staff:<id>:portrait` / `character:<id>:portrait`.
  Covers are cache-first: a hit skips both the network and the semaphore.
- **Sharing**: keys are global, not scoped per account. The data is public
  catalog data; the only variant is the viewer list entry inside `anime:<id>`,
  which a successful request refreshes when logged in.
- **Durability**: every write is an LMDB transaction, so an interrupted write
  cannot leave a truncated entry.
- Every cache operation is best effort: failures are ignored and never turn
  into a request error.
- **Growth** is unbounded: no eviction and no per-entry cap. The whole
  environment maps 1 GiB, which also covers login, progress and pending data.

The cache is read and written by worker tasks, never by the render path. It does
not make the TUI an offline mode: there is no prefetch, and only data already
fetched is available.

### Execution model

One single loop owns the UI state. It never awaits network I/O.

```text
        +---------------------------------------------+
        |  main loop (single task)                    |
        |                                             |
  input |  select! { biased;                          |
------->|    terminal events    -> app.on_key()       |----> terminal.draw(if dirty)
  net   |    net messages       -> app.on_net()       |
------->|    tick (100ms)       -> app.tick()         |
        +---------------------------------------------+
                          |  spawn(cancellable task)
                          v
        +---------------------------------------------+
        |  worker tasks (tokio)                       |
        |  reqwest + spawn_blocking for image work    |
        |  semaphores: graphql(4), covers(3)          |
        +---------------------------------------------+
```

Rules:

1. `terminal.draw` is only called when `App::dirty` is set. No unconditional
   redraw at 60 fps.
2. Network futures are never awaited in the loop. They are spawned.
3. `select!` is `biased` with terminal input first, so a keypress is handled
   before a queued network message in the same wakeup.
4. Rendering performs no blocking work: images are downloaded and encoded
   off-thread, and the render path only reads an `Arc<Protocol>` from cache.

### Scope / request model

Cancellation is tied to the widget lifecycle, not to a single request.

- **Scope** = one mounted view instance: the trending/search/genre list, one
  open media page, or one person page. Scopes get a monotonic `ScopeId` and own
  a parent `CancellationToken`.
- **Request** = `(ScopeId, ReqKey)`. A scope can have many requests in flight at
  the same time (list page, several covers, detail cover).

`ReqKey` identifies the data class: `Trending(page)`, `Search(query, page)`,
`Browse(filter, page)`, `Detail(id)`, `Credits(kind, id, page)`,
`Person(kind, id, page)`, `Cover(url, w, h)`.

Cancellation rules:

| Event | Effect |
| --- | --- |
| Widget unmounted (source change, media page closed) | Retire scope: cancel parent token, abort every child task |
| Request with the same `(scope, key)` re-issued | Cancel previous request only |
| Re-render, scroll within page | Nothing is cancelled |
| Search text changes | Debounced 200 ms; then the search scope is retired and remounted |

Double protection against stale data:

1. `CancellationToken` in the worker `select!` plus `JoinHandle::abort()` as a
   hard fallback.
2. Every message carries `(scope, req_id)`. `App` drops it when the scope is not
   live or the req id is no longer current. This covers the race where the abort
   loses against a response already in the channel. Every received message also
   clears its entry in `Net::aborts`, so the header's in-flight counter reflects
   running work, not history.

A cancelled request produces no user-visible error and no message.

### Networking

- AniList only speaks GraphQL: single POST endpoint. Queries are written as
  strings and deserialized into partial `serde` structs. `graphql_client` is not
  used: it needs a local SDL schema which AniList does not publish, and a
  hand-written query per view is simpler than a build step.
- Client-side rate limiter: token bucket, burst 10, refill 1 every 600 ms
  (about 100 req/min, above the server's 90/min only in short bursts). The
  server may still count a cancelled request, so requests are debounced and
  results cached.
- `reqwest::Client` is shared, HTTP/2, connection pooling on.

### Caches

- `detail_data: Option<MediaDetail>`, dropped as soon as the page closes.
- `covers: HashMap<(url, cols, rows), Arc<Protocol>>`, app-owned. Image encodings
  are fixed size, so a cached entry is rendered with zero work.
- `pending_covers: HashSet<key>` prevents duplicate downloads of the same cover
  while one is in flight.
- Both cleared/kept per scope policy: cache entries survive scope retirement,
  pending markers do not.
- `failed_covers: HashSet<key>` keeps a broken URL from being retried on every
  frame. Cleared when the view is replaced.

No TTL for the in-memory caches: they live for one process, and disk cache
freshness is handled by the per-type TTLs above.

### Image pipeline

```text
draw sees a missing cover (App::cover(url, w, h))
   -> disk cache hit?           (raw bytes, no network)
      no -> reqwest GET bytes   (semaphore 3), then write cache
   -> spawn_blocking(decode + encode)     image crate + picker.new_protocol
   -> NetMsg::Cover(Arc<Protocol>)
   -> App inserts into covers cache, marks dirty
   -> draw: render_widget(Image::new(&proto), centered rect)
```

`Picker::from_query_stdio()` runs once, before raw mode / alternate screen, and
queries the terminal for its graphics protocol and font size. If the query
fails, `Picker::halfblocks()` is used with a default font size.

Encoded images are cell-exact, so a terminal resize does not invalidate them.
Only the layout changes.

### Infinite scroll

The list never replaces its scope to paginate. When the selection gets within
`PREFETCH_AHEAD` (5) rows of the end, `maybe_prefetch` increments the page and
requests the next one on the **same scope**, so covers already loading stay
alive. Messages carry their page number: page 1 replaces the items, any later
page appends. `has_next == false` or an in-flight request stops the prefetch.
A source change, a search change or `r` retire the scope and restart from page 1.

### Media page

Full screen, two independent interactive columns:

- **Sidebar** (`ui/page.rs::sidebar_items`): cover as the first item (18 rows), so
  it scrolls away with the rest, then every available field and the tag list.
  Spoiler tags are filtered out unless `App::show_spoiler_tags` is set (`s`); the
  sidebar shows how many are hidden. Visible tags are packed greedily into lines
  (`pack_tags`) and each keeps its own style: cyan normally, red for spoilers. There
  is no `*` marker.
- **Content**, by tab (`App::tab`). The tab bar is drawn at the top of the content
  column only, so the sidebar keeps its own layout and does not look like part of
  the tab switch:
  - `Overview` (`content_items`): title, facts, genres, synopsis, relations,
    the first 12 characters and staff, then recommendations (AniList's order). Credits are
    `Item::Person` rows (4x3 portrait, name, role/voice actor): informational
    only, they register no link and the keyboard cursor skips them.
  - `Characters` / `Staff` (`credit_items`): the full list. Each holds a
    `CreditList { items, page, has_next, loading }` (no total: AniList's is
    bogus) fed by paginated
    `Job::Credits` requests (25 per page) on the same detail scope. The renderer
    asks for the next page (`App::prefetch_credits`) when the scroll gets within
    5 rows of the end, and the request is cancelled with the page like everything
    else.
  - Switching tabs resets `content_scroll` and requests page 1 the first time a
    credit tab is opened. `h`/`l`, arrows and `1`/`2`/`3` switch tabs; they never
    change the selected media. The tab labels record their rects in
    `App::tab_targets` on every draw, so they are clickable: `App::on_mouse`
    checks them before the page links.

Each column is a virtual list: every item declares a height (cover 18, entry 3,
text 1), so offsets are exact and only visible items are rendered. Offsets live
in `App::sidebar_scroll` and `App::content_scroll`, clamped in `draw` against the
real content height. `App::page_focus` (`Tab`) selects which column the keys and
the wheel scroll; the top bar shows the focused pane. Leaving the page
(`Esc`/`Backspace`) resets both offsets.

Opening the page mounts a dedicated scope and issues `Job::Detail(id)`; the
`MediaDetail` response carries the full media record plus `relations`,
`recommendations`, `characters` and `staff`. The list query stays lean: that data is fetched once, only
when a page is actually read, and the request is cancelled by the same scope
rules as everything else.

Wheel events are handled before clicks: on the list they move the selection,
which drives the viewport; on the media page they scroll the focused column.
Three rows per notch.

List rows are clickable too: `ui/list.rs` records `(rect, media_id)` in
`App::list_targets` for every visible row, and a left click opens that media's
page (and moves the selection to it). `App::on_mouse` dispatches on the current
screen: list targets when the list is visible, page targets when a media page is
open.

Everyone clickable on the page is a `PageLink`: `Media(Box<Media>)`,
`Genre(String)` or `Tag(String)`. Genres (content column) and tags (sidebar) are
rendered as chips packed greedily into lines, and each chip registers its own
rect.

Characters and staff are `PageLink::Character(id)` / `PageLink::Staff(id)`, in the
overview and in their tabs. Both open the same screen: `PersonView { data, page,
loading, error, scroll, cursor, reveal, cursor_visible }` holding a
`PersonPageData { kind, fields, sections }`, so one renderer (`ui/person.rs`)
serves both. It is a separate screen with its own scope: opening it mounts the scope and requests `Job::Staff`,
closing it (`Esc`/`Backspace`) retires the scope and returns to the media page,
which stays open underneath. Links follow the active tab: the Staff tab exposes
its own paginated rows, the Characters tab exposes none.

On the person page every collaboration is a `Row::Work` with a 6x3 thumbnail and a
`PageLink::Media`, so it is clickable and reachable with `n`/`p` + `Enter` (same
reveal/visibility rules as the media page). Opening a work closes the person page
first, then opens the media page with the origin media pushed onto
`page_history`, so `Backspace` walks back.

Keyboard access uses one cursor per column: `App::link_cursor` for the content
(genres, relations, recommendations, in render order) and `App::tag_cursor` for
the sidebar tags. `n`/`p` step the cursor of the focused column only. The renderer
highlights the focused link.

Scrolling stays free while a link is focused: the page scrolls the focused
column into view only once, right after the cursor moved (`App::reveal_cursor`),
and never again on the following frames. Each draw reports back whether the
focused link was actually rendered (`App::cursor_visible`); an off-screen link
stays selected but `Enter` is ignored for it. `Enter` follows the focused link: `Media`
opens the page (same path as a click), `Genre`/`Tag` leave the page and switch the
list to `Source::Genre` / `Source::Tag`, which requests `Job::Browse` — a
server-side `genre:` / `tag:` filter sorted by popularity.

Relations and recommendations entries are clickable: each render pass records
`(rect, media_id)` in `App::page_targets`, and `App::on_mouse` hit-tests a left
click against those rects. Mouse capture is enabled in `term.rs`. A click on a
related entry pushes the current page onto `App::page_history`, mounts a new page
scope for that media and refetches it in full. `Backspace`/`Esc` pop one entry and
reload that page (also refetched); with an empty history they close the page and
return to the list. Opening a page from the list starts with an empty history.

Horizontal navigation is deliberately disabled on this screen: arrow left/right
never change the selected media.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | CLI parsing, command dispatch, sync loop, login |
| `src/api.rs` | blocking AniList GraphQL client (`ureq`), auth header, progress mutation |
| `src/database.rs` | LMDB storage, login, episode map, pending queue, TUI cache database, `SyncContext` |
| `src/config.rs` | TOML config from the platform config dir (TUI only) |
| `src/daemon.rs` | unix double-fork detach for background sync |
| `src/tui/mod.rs` | TUI entry, stored-login lookup, tokio runtime, event loop, `select!`, picker |
| `src/tui/cache.rs` | persistent GraphQL response and cover cache |
| `src/tui/term.rs` | terminal init/restore, mouse capture, panic hook |
| `src/tui/app.rs` | UI state, input handling, message handling, debounce, cache |
| `src/tui/model.rs` | `Media`, `Title`, `CoverImage` |
| `src/tui/net/mod.rs` | scopes, cancellable spawn, `Net`, `NetMsg`, `Job` |
| `src/tui/net/graphql.rs` | query strings, request/response types |
| `src/tui/net/rate.rs` | token bucket |
| `src/tui/ui/mod.rs` | layout, header/footer, search bar |
| `src/tui/ui/list.rs` | vertical list: cover left, title/meta/synopsis right |
| `src/tui/ui/page.rs` | full screen media page: top-left cover, left sidebar with all fields and tags, right column with title/genres/synopsis |
| `src/tui/ui/person.rs` | staff and character page: portrait/profile left, paginated filmography right |
| `src/tui/ui/text.rs` | truncation and HTML stripping helpers |

## Invariants

1. No `.await` on network in `src/tui/app.rs` or `src/tui/ui/*`.
2. No blocking work in the render path; image decode/encode only in
   `spawn_blocking`.
3. Every spawned request is owned by a live scope and reachable by
   `Net::cancel_all` on retirement. No detached task without a token.
4. A render pass never mutates cancellation state. It may only enqueue requests.
5. Stale messages are dropped silently, never rendered as errors.
6. The TUI only reads the stored login and local progress at startup; it never
   writes account state.
7. Cache reads and writes are best effort: a cache failure never fails a
   request. Fetched GraphQL bodies and covers stay readable offline.

## Testing

- Unit: scope bookkeeping (`request` supersedes, `retire` cancels and aborts,
  unknown scope ignored), stale `req_id` rejection, token bucket refill,
  rendering helpers, key/mouse routing, config parsing, the `e` target, and the
  CLI defaulting to `tui` on a bare invocation.
- Manual: latency under load, cancellation behaviour while spamming `j`, search
  typing, terminal resize, half-blocks fallback, and the external command
  suspension (terminal handed to the child and restored afterwards).
- CLI: no automated coverage. Database behaviour is checked manually with
  `scrobble --local-only` followed by `sync`.

A fake slow HTTP server test is intentionally out of the PoC: the determinism
lives in the bookkeeping, not in the socket.
