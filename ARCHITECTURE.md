# Architecture

One binary, two front ends over the same AniList account:

- **CLI** (always built): `login`, `sync`, `scrobble`. Tracks episodes locally
  and pushes progress to AniList.
- **TUI** (`tui` feature, on by default, `tui` subcommand): read-only AniList
  catalog browser. The subcommand is also the default when no subcommand is
  given. `--no-default-features` builds the CLI alone.

The CLI is blocking and single-threaded. The TUI is async and input-first. They
share the crate, not runtime state: the TUI does not read the database and needs
no login.

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

### Goal

Responsiveness first. User input has absolute priority over network work. A
request that is no longer useful is cancelled, not awaited and discarded later
only.

Non-goals for the TUI: authentication, persistence of the catalog, background
sync. The CLI owns the account data.

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

No TTL for now: the process is short-lived, and a stale catalog entry is
harmless.

### Image pipeline

```text
draw sees a missing cover (App::cover(url, w, h))
   -> reqwest GET bytes          (semaphore 3)
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
| `src/database.rs` | LMDB storage, login, episode map, pending queue, `SyncContext` |
| `src/daemon.rs` | unix double-fork detach for background sync |
| `src/tui/mod.rs` | TUI entry, tokio runtime, event loop, `select!`, picker |
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
6. The TUI never reads the database. Account state belongs to the CLI.

## Testing

- Unit: scope bookkeeping (`request` supersedes, `retire` cancels and aborts,
  unknown scope ignored), stale `req_id` rejection, token bucket refill,
  rendering helpers, key/mouse routing, and the CLI defaulting to `tui` on a
  bare invocation.
- Manual: latency under load, cancellation behaviour while spamming `j`, search
  typing, terminal resize, half-blocks fallback.
- CLI: no automated coverage. Database behaviour is checked manually with
  `scrobble --local-only` followed by `sync`.

A fake slow HTTP server test is intentionally out of the PoC: the determinism
lives in the bookkeeping, not in the socket.
