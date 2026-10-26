# Roadmap

Two goals, one binary:

- CLI: never lose an episode you scrobbled, even offline or with the network
  down. Local progress first, sync later.
- TUI: prove that a cover-heavy TUI over a remote GraphQL API can stay
  input-first, by cancelling work that the user has moved away from.

## CLI

- [x] LMDB storage: versioned `main` db (`login`, `pending`) plus `id -> episode`.
- [x] `login` with `--force`: validate the pasted OAuth implicit token against
  `Viewer { id }`, store token and user id.
- [x] `scrobble`: write local progress only when it moves forward, enqueue the
  id, never touch the network with `--local-only`.
- [x] `sync`: fetch public episode count and authenticated progress, push the
  higher local value, mark `COMPLETED` when the last episode is reached.
- [x] Resumable queue: an id leaves `pending` only after AniList accepted it.
- [x] Background sync: unix double fork, detached process on Windows.
- [x] Non-fatal API errors: report and skip one entry, keep the rest.

Acceptance: `scrobble --local-only` works with no network; a later `sync`
pushes the queued episode; interrupting `sync` leaves the entry queued; running
`scrobble` with an older episode never lowers progress.

## TUI

### Milestone 1 - Skeleton

- [x] Project layout, `Cargo.toml`, docs.
- [x] Main loop with biased `select!`.
- [x] Terminal init/restore, panic-safe.
- [x] Placeholder list, selection movement, page math.
- [x] Scope bookkeeping (`mount`, `retire`, `request`) with unit tests.

Acceptance: scrolling is instant with no network involved; `Ctrl-C` always
restores the terminal; `q` exits.

### Milestone 2 - Trending

- [x] GraphQL client: shared `reqwest::Client`, user agent, error mapping.
- [x] Token bucket rate limiter.
- [x] `Trending(page)` query, list rendering.
- [x] Infinite scroll: the next page is appended to the same scope when the
  selection nears the end, no page navigation.

Acceptance: holding `j` never queues work; changing source while loading cancels
the previous scope; a failed request shows a retryable error, `r` retries.

### Milestone 3 - Covers

- [x] `Picker::from_query_stdio()` with half-blocks fallback and
  `ANISCROBBLE_TUI_PROTOCOL` override.
- [x] Cover download with a dedicated semaphore.
- [x] Decode + encode inside `spawn_blocking`.
- [x] `Arc<Protocol>` cache keyed by `(url, cols, rows)`.
- [x] Only request covers for cells actually visible.

Acceptance: covers appear progressively, input stays responsive during
downloads, no duplicate requests for the same cover, no re-encode on scroll.

### Milestone 4 - Search and detail

- [x] Search input with 200 ms debounce.
- [x] `Search(query, page)` query, same list rendering.
- [x] Committing a new query retires the previous search scope.
- [x] Full screen media page opened from already loaded data.

Acceptance: fast typing issues one request; closing the panel or changing
source cancels its pending cover; `Tab` returns to trending without refetching
cached covers.

### Milestone 4b - List and page layout

- [x] Vertical rows: cover left, title + year + genres + synopsis right.
- [x] Genres cut to a single line.
- [x] Enter opens a full screen page modelled on the AniList web page:
  top-left cover, left sidebar with every available field (format, episodes,
  duration, status, dates, season, average/mean score, popularity, favourites,
  source, country, studios, producers, hashtag, synonyms) and the full tag list,
  right side with title, genres and synopsis.
- [x] Tabs live at the top of the content column only, not across the page, and
  are clickable with the mouse.
- [x] Media page split in sidebar and content: `Tab` switches focus, each column
  scrolls independently, the cover scrolls with the sidebar, offsets clamped to
  content. Left/right disabled, Backspace and Esc go back, Enter does nothing.
- [x] Relations and recommendations on the page, loaded by a page-scoped
  `Detail` query that is cancelled when the page closes.
- [x] Thumbnails on every related entry, click to open that media (mouse
  capture, hit test on rects recorded during render).
- [x] Clickable rows in trending and search results.
- [x] Keyboard link cursor on the media page: `n`/`p` move it and scroll it into
  view once; scrolling stays free afterwards and an off-screen link cannot be
  opened.
- [x] Spoiler tags hidden by default, `s` toggles them, spoilers are red (no `*`
  marker).
- [x] Genres and tags are clickable chips (and reachable with `n`/`p` + `Enter`):
  they browse the catalog through a server-side `genre:`/`tag:` filter, and
  `Backspace` returns to the page with its history intact.
- [x] Mouse wheel scrolls the list and the media page.
- [ ] Banner image on the media page (needs `bannerImage` in the query).
- [x] Characters and staff sections on the overview, with portraits and roles,
  from the same `Detail` query.
- [x] Dedicated Characters and Staff tabs listing every credit with pagination
  (25 per page, infinite scroll) and Japanese voice actors. No totals are shown:
  `pageInfo.total` is unreliable here (see `docs/anilist-api.md`), only the
  loaded count is displayed.

### Milestone 5 - Polish

- [x] Error states with retry per view.
- [x] Loading indicator in the header.
- [x] Empty state for searches with no results.
- [ ] Half-blocks legibility pass (fewer columns per cell when needed).
- [x] Page history: clicking a related entry pushes the current page, and
  `Esc`/`Backspace` walk back one page at a time before returning to the list.
- [x] Person page for staff and characters: profile plus paginated filmography,
  opened from their rows, closed back to the media page. Every collaboration has
  a thumbnail and opens its anime (click or `n`/`p` + `Enter`).

### Packaging inside aniscrobble

- [x] TUI source imported under `src/tui/`, crate paths rewritten.
- [x] `tui` subcommand behind the `tui` feature, which is on by default;
  optional dependencies only, so `--no-default-features` builds the CLI alone.
- [x] Bare invocation defaults to `tui` when the feature is on.
- [x] TUI unit tests run under the default `cargo test`.
- [x] Account info: read the CLI's stored login, authenticate the detail query,
  show the viewer's status, score, progress, rewatch count and notes on the
  media page. Local pending progress wins over a lower AniList value. Read-only,
  no rows when logged out.
- [x] README, architecture, agent rules and API notes live in this repository.

## Later, explicitly out of scope

- Mutations, login or persistence inside the TUI.
- Prefetch of the next page, hover prefetch of details.
- Windows support for the TUI.
- Cover cache persisted across runs.
