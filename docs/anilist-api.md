# AniList API notes

Reference for the parts this project uses. Verify against the live API before
trusting anything here blindly.

Two clients hit the same endpoint: the CLI through blocking `ureq`, the TUI
through `reqwest`. Queries and limits are shared.

## Endpoint

- Single endpoint: `POST https://graphql.anilist.co`
- Body: `{ "query": "...", "variables": { ... } }`
- Headers: `Content-Type: application/json` and an explicit `User-Agent`.
  Cloudflare in front of AniList rejects some default user agents. The CLI sets
  both headers by hand; the TUI's JSON body sets the content type.
- Auth header when a token is available: `Authorization: Bearer <token>`.
- No REST. No API key for public data.

## Response shape

```json
{
  "data": { "Page": { ... } },
  "errors": [{ "message": "Validation error", "status": 400, "locations": [...] }]
}
```

`errors` can be present with a non-2xx status. `data` can be null. Treat both as
an application error with a readable message.

## Media fields used

```graphql
id
title { romaji english }
coverImage { large }
format
seasonYear
averageScore
episodes
genres
description(asHtml: false)
```

`description` contains HTML tags even with `asHtml: false`; strip or ignore.
Nullable: `format`, `seasonYear`, `averageScore`, `episodes`, `description`.
Cover URL points at the AniList CDN, not at the API host, so it is not subject
to the API rate limit.

## Queries

Trending:

```graphql
query Trending($page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(type: ANIME, sort: TRENDING_DESC) {
      id
      title { romaji english }
      coverImage { large }
      format
      seasonYear
      averageScore
      episodes
      genres
      description(asHtml: false)
    }
  }
}
```

Search:

```graphql
query Search($search: String, $page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(type: ANIME, search: $search, sort: SEARCH_MATCH) {
      id
      title { romaji english }
      coverImage { large }
      format
      seasonYear
      averageScore
      episodes
      genres
      description(asHtml: false)
    }
  }
}
```

`perPage` max is 50; this app uses 30.

Page detail query: the full media field set (same fields as the list query) plus
relations and recommendations. Fetched only when a media page opens, and again
when a related entry is clicked.

```graphql
query Detail($id: Int, $type: MediaType) {
  Media(id: $id, type: $type) {
    # ...the full field set used by the list query...
    mediaListEntry {
      status
      score
      progress
      repeat
      notes
    }
    relations {
      edges {
        relationType
        node {
          id
          title { romaji english }
          coverImage { large }
          format
          seasonYear
          averageScore
          episodes
        }
      }
    }
    recommendations(sort: RATING_DESC, perPage: 12) {
      nodes {
        mediaRecommendation {
          id
          title { romaji english }
          coverImage { large }
          format
          seasonYear
          averageScore
          episodes
        }
      }
    }
  }
}
```

`mediaListEntry` is viewer-specific: it is `null` without a token and `null`
when the media is not on the user's list. `score` is a `Float` whose scale
follows the user's AniList settings (100, 10-decimal, 5 stars, 3-point), so show
it raw rather than appending a fixed `/ 100`. `notes` can be `null` or contain
newlines.

`coverImage` is required on the nodes: the page shows a thumbnail for every
relation and recommendation.

Characters and staff are part of the same detail query:

```graphql
characters(sort: [ROLE, RELEVANCE, ID], perPage: 12) {
  edges {
    role            # MAIN | SUPPORTING | BACKGROUND
    node { name { full } image { large } }
  }
}
staff(sort: [RELEVANCE, ID], perPage: 12) {
  edges {
    role            # free text: "Original Creator", "Director", ...
    node { name { full } image { large } }
  }
}
```

`node`, `node.name`, `node.image` and `image.large` can all be `null` (staff
portraits often are). Deserialization must tolerate null, not just missing keys:
`#[serde(default)]` alone is not enough for an object field.

Cost: 12 characters + 12 staff per page add a few KB to the detail response, no
extra request. Do not ask for `pageInfo { total }` here: it is unreliable (see
the gotcha below), so the overview lists the first rows without a total.

The dedicated tabs paginate the same connections, 25 rows per page:

```graphql
query Characters($id: Int, $page: Int, $perPage: Int) {
  Media(id: $id, type: ANIME) {
    characters(page: $page, perPage: $perPage, sort: [ROLE, RELEVANCE, ID]) {
      pageInfo { total hasNextPage }
      edges {
        role
        voiceActors(language: JAPANESE) { name { full } }
        node { name { full } image { large } }
      }
    }
  }
}
```

The staff variant is the same with `staff(page: $page, perPage: $perPage, sort:
[RELEVANCE, ID])` and no `voiceActors`. `perPage` maxes at 50; this app uses 25.

Gotcha: `pageInfo.total` is **unreliable** for `characters` and `staff`. It
reported 500/500 for Cyberpunk: Edgerunners 2 and One Piece, and 500 for series
that really have 14 characters and 18 staff. Do not show it: the only trustworthy
signal is `hasNextPage`, so the UI displays how many rows are loaded and keeps
paginating until `hasNextPage` is false.

Gotcha: `pageInfo` is camelCase in JSON. It must be requested explicitly (`total`
is not included by default) and mapped with `#[serde(rename = "pageInfo")]`,
otherwise `total` and `hasNextPage` silently deserialize to zero/false. The same
applies to `voiceActors`, whose items are full `Character`/`Staff` objects
(`{ name { full } }`), not `{ full }` directly.

`relationType` values include `ADAPTATION`, `PREQUEL`, `SEQUEL`, `PARENT`,
`SIDE_STORY`, `CHARACTER`, `SUMMARY`, `ALTERNATIVE`, `SPIN_OFF`, `OTHER`,
`SOURCE`, `COMPILATION`, `CONTAINS`. `node` and `mediaRecommendation` can be
`null`.

Genre / tag browse, used when a genre or tag chip is followed:

```graphql
query Browse($genre: String, $tag: String, $page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(type: ANIME, genre: $genre, tag: $tag, sort: POPULARITY_DESC, isAdult: false) {
      # ...same field set as the list query...
    }
  }
}
```

`genre` and `tag` are single `String` arguments; this app sends exactly one of
them. `Popularity: DESC` matches the order AniList uses on its genre pages closely
enough for a TUI. Filtering happens server side, so a genre page is a normal
paginated list that the infinite scroll can extend.

Staff page, including the paginated filmography:

```graphql
query StaffPage($id: Int, $page: Int, $perPage: Int) {
  Staff(id: $id) {
    id
    name { full }
    image { large }
    description(asHtml: false)
    languageV2
    homeTown
    yearsActive
    primaryOccupations
    favourites
    dateOfBirth { year month day }
    staffMedia(page: $page, perPage: $perPage, sort: [START_DATE_DESC]) {
      pageInfo { hasNextPage }
      edges {
        staffRole
        node { id title { romaji english } coverImage { large } format seasonYear }
      }
    }
    characterMedia(page: $page, perPage: $perPage, sort: [START_DATE_DESC]) {
      pageInfo { hasNextPage }
      edges {
        characters { name { full } }
        node { id title { romaji english } coverImage { large } format seasonYear }
      }
    }
  }
}
```

Both connections are paged with the same `page` variable, so one request advances
both lists. Directing-only staff often have an empty `characterMedia`.

Character page:

```graphql
query CharacterPage($id: Int, $page: Int, $perPage: Int) {
  Character(id: $id) {
    id
    name { full }
    image { large }
    description(asHtml: false)
    gender
    age            # String, can be empty
    bloodType
    favourites
    dateOfBirth { year month day }
    media(page: $page, perPage: $perPage, sort: [START_DATE_DESC]) {
      pageInfo { hasNextPage }
      edges {
        characterRole  # MAIN | SUPPORTING | BACKGROUND
        node { id title { romaji english } coverImage { large } format seasonYear }
      }
    }
  }
}
```

## Rate limit

- 90 requests per minute, degrading to 30/min if exceeded.
- Response headers: `X-RateLimit-Limit`, `X-RateLimit-Remaining`,
  `X-RateLimit-Reset`.
- A request cancelled client-side may still be counted by the server. Debounce
  input and cache results; do not rely on cancellation to save quota.
- Both clients obey the same limit. The TUI token bucket is
  `src/tui/net/rate.rs`; the CLI has no limiter because it sends a handful of
  sequential requests per pending entry.

## Auth

Used by the CLI. The TUI is public-only.

OAuth2 implicit grant:

```text
https://anilist.co/api/v2/oauth/authorize?client_id=7723&response_type=token
```

The token comes back in the URL fragment. Send it as
`Authorization: Bearer <token>`. Without a token the API serves public data only.

`login` validates the token and stores the user id:

```graphql
query {
  Viewer { id }
}
```

## Current lists

The home screen lists the viewer's in-progress entries through `Page.mediaList`.
`userId` is the id stored at login, `type` is `ANIME` or `MANGA`, and
`status: CURRENT` keeps the query to the two sections AniList shows.

```graphql
query MyList($userId: Int, $type: MediaType, $page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    mediaList(
      userId: $userId
      type: $type
      status: CURRENT
      sort: UPDATED_TIME_DESC
    ) {
      progress
      media {
        id
        type
        title { romaji english }
        coverImage { large extraLarge }
        format
        episodes
        chapters
        status
        nextAiringEpisode { episode }
      }
    }
  }
}
```

`nextAiringEpisode.episode` is the next episode to air, so `episode - 1` is what
is already available. Finished media fall back to `episodes` (anime) or
`chapters` (manga). There is no equivalent "next chapter" field, so an ongoing
manga cannot report new chapters reliably; only finished manga use `chapters`.

## Sync queries

Episode count for one media, public:

```graphql
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    episodes
  }
}
```

Current progress, authenticated. A `404`/missing entry means the anime is not on
the user's list: treat progress as 0.

```graphql
query ($userId: Int, $mediaId: Int) {
  MediaList(userId: $userId, mediaId: $mediaId, type: ANIME) {
    progress
  }
}
```

## Mutations

Used by the CLI `sync`.

- `SaveMediaListEntry(mediaId, status, score, progress, notes, private)`.
- `DeleteMediaListEntry(id)`.
- `MediaList` statuses: `CURRENT`, `PLANNING`, `COMPLETED`, `DROPPED`, `PAUSED`,
  `REPEATING`.

The progress mutation. `status` is `COMPLETED` when `progress` reaches the
episode count, `CURRENT` otherwise:

```graphql
mutation ($mediaId: Int, $status: MediaListStatus, $progress: Int) {
  SaveMediaListEntry(mediaId: $mediaId, status: $status, progress: $progress) {
    progress
  }
}
```

## Other public shapes

`Media(type: MANGA)`, `airingSchedule`, `Page.media(isAdult: false)`,
`Viewer`, `Studio`, `Character`, `Staff`. Useful for later screens.
