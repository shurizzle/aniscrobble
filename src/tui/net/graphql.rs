//! GraphQL access. Queries are plain strings with partial `serde` types on
//! purpose: AniList publishes no SDL, so codegen would need an introspection
//! step for three queries.

use anyhow::{Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use crate::tui::cache::Cache;
use crate::tui::model::{CreditImage, CreditName, Media, MediaDetail, MediaKind, MyListEntry};

const API: &str = "https://graphql.anilist.co";
pub const USER_AGENT: &str = concat!("aniscrobble-tui/", env!("CARGO_PKG_VERSION"));

const DETAIL: &str = r#"
query Detail($id: Int, $type: MediaType) {
  Media(id: $id, type: $type) {
    FIELDS
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
          RELATION_FIELDS
        }
      }
    }
    recommendations(sort: RATING_DESC, perPage: 12) {
      nodes {
        mediaRecommendation {
          RELATION_FIELDS
        }
      }
    }
    characters(sort: [ROLE, RELEVANCE, ID], perPage: 12) {
      edges {
        role
        voiceActors(language: JAPANESE) { name { full } }
        node { id name { full } image { large } }
      }
    }
    staff(sort: [RELEVANCE, ID], perPage: 12) {
      edges {
        role
        node { id name { full } image { large } }
      }
    }
  }
}
"#;

const MEDIA_FIELDS: &str = r#"
      id
      title { romaji english }
      coverImage { large extraLarge }
      format
      seasonYear
      averageScore
      episodes
      duration
      status
      popularity
      favourites
      studios(isMain: true) { nodes { name } }
      producers: studios(isMain: false) { nodes { name } }
      season
      meanScore
      source
      countryOfOrigin
      hashtag
      synonyms
      tags { name rank isMediaSpoiler }
      startDate { year month day }
      endDate { year month day }
      genres
      description(asHtml: false)
      type
      chapters
      volumes
      nextAiringEpisode { episode }
"#;

const RELATION_FIELDS: &str = r#"
          id
          type
          title { romaji english }
          coverImage { large }
          format
          seasonYear
          averageScore
          episodes
"#;

pub const TRENDING: &str = r#"
query Trending($page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(type: ANIME, sort: TRENDING_DESC, isAdult: false) {
      FIELDS
    }
  }
}
"#;

pub const BROWSE: &str = r#"
query Browse($genre: String, $tag: String, $page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(type: ANIME, genre: $genre, tag: $tag, sort: POPULARITY_DESC, isAdult: false) {
      FIELDS
    }
  }
}
"#;

pub const SEARCH: &str = r#"
query Search($search: String, $page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(type: ANIME, search: $search, sort: SEARCH_MATCH, isAdult: false) {
      FIELDS
    }
  }
}
"#;

const MY_LIST: &str = r#"
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
        FIELDS
      }
    }
  }
}
"#;

#[derive(Serialize)]
struct Body<'a> {
    query: &'a str,
    variables: Value,
}

#[derive(Clone, Copy)]
pub enum CacheMode<'a> {
    /// Serve a fresh entry without a request.
    LocalFirst(&'a Cache, &'a str, Duration),
    /// Always request; use the entry only when the request fails.
    NetworkFirst(&'a Cache, &'a str),
}

type CacheRef<'a> = Option<CacheMode<'a>>;

/// Send one GraphQL request and persist a successful body under `key`. A fresh
/// cached body is returned without touching the network. On a transport or HTTP
/// failure, the cached body is used even when expired, so everything already
/// fetched stays readable while offline.
async fn fetch(client: &Client, cache: CacheRef<'_>, body: &Body<'_>) -> Result<(String, String)> {
    fetch_from(API, client, cache, body).await
}

async fn fetch_from(
    api: &str,
    client: &Client,
    cache: CacheRef<'_>,
    body: &Body<'_>,
) -> Result<(String, String)> {
    if let Some(CacheMode::LocalFirst(cache, key, ttl)) = cache
        && let Some(cached) = cache.read_fresh(key, ttl)
    {
        return Ok((
            "cache".to_string(),
            String::from_utf8_lossy(&cached).into_owned(),
        ));
    }

    let request = client
        .post(api)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(body);
    let fallback = || {
        cache
            .and_then(|mode| match mode {
                CacheMode::LocalFirst(cache, key, _) | CacheMode::NetworkFirst(cache, key) => {
                    cache.read_any(key)
                }
            })
            .map(|cached| {
                (
                    "cache".to_string(),
                    String::from_utf8_lossy(&cached).into_owned(),
                )
            })
    };

    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let text = response.text().await?;
            if status.is_success() {
                if let Some(cache) = cache {
                    match cache {
                        CacheMode::LocalFirst(cache, key, _)
                        | CacheMode::NetworkFirst(cache, key) => {
                            cache.write(key, text.as_bytes());
                        }
                    }
                }
                Ok((status.as_u16().to_string(), text))
            } else {
                Ok(fallback().unwrap_or_else(|| (status.as_u16().to_string(), text)))
            }
        }
        Err(err) => fallback().ok_or_else(|| err.into()),
    }
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct Data {
    #[serde(rename = "Page", default)]
    page: Option<Page>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    #[serde(default)]
    media: Vec<Media>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

pub fn trending_query() -> String {
    TRENDING.replace("FIELDS", MEDIA_FIELDS)
}

pub fn search_query() -> String {
    SEARCH.replace("FIELDS", MEDIA_FIELDS)
}

pub fn browse_query() -> String {
    BROWSE.replace("FIELDS", MEDIA_FIELDS)
}

pub fn my_list_query() -> String {
    MY_LIST.replace("FIELDS", MEDIA_FIELDS)
}

/// Paginated full credit lists: the media page has dedicated tabs for them.
const CHARACTERS: &str = r#"
query Characters($id: Int, $type: MediaType, $page: Int, $perPage: Int) {
  Media(id: $id, type: $type) {
    characters(page: $page, perPage: $perPage, sort: [ROLE, RELEVANCE, ID]) {
      pageInfo { hasNextPage }
      edges {
        role
        voiceActors(language: JAPANESE) { name { full } }
        node { id name { full } image { large } }
      }
    }
  }
}
"#;

const STAFF: &str = r#"
query Staff($id: Int, $type: MediaType, $page: Int, $perPage: Int) {
  Media(id: $id, type: $type) {
    staff(page: $page, perPage: $perPage, sort: [RELEVANCE, ID]) {
      pageInfo { hasNextPage }
      edges {
        role
        node { id name { full } image { large } }
      }
    }
  }
}
"#;

#[derive(Deserialize)]
struct CreditsResponse {
    #[serde(default)]
    data: Option<CreditsData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct CreditsData {
    #[serde(rename = "Media", default)]
    media: Option<CreditsMedia>,
}

#[derive(Deserialize)]
struct CreditsMedia {
    #[serde(default)]
    characters: crate::tui::model::CreditConnection,
    #[serde(default)]
    staff: crate::tui::model::CreditConnection,
}

/// One page of the full character or staff list.
pub async fn media_credits(
    client: &Client,
    cache: CacheRef<'_>,
    kind: crate::tui::model::CreditKind,
    media_kind: MediaKind,
    id: i64,
    page: u32,
    per_page: u32,
) -> Result<(Vec<crate::tui::model::Credit>, bool)> {
    let query = match kind {
        crate::tui::model::CreditKind::Characters => CHARACTERS,
        crate::tui::model::CreditKind::Staff => STAFF,
    };

    let (status, text) = fetch(
        client,
        cache,
        &Body {
            query,
            variables: serde_json::json!({
                "id": id,
                "type": media_kind.api_name(),
                "page": page,
                "perPage": per_page,
            }),
        },
    )
    .await?;

    let parsed: CreditsResponse =
        serde_json::from_str(&text).map_err(|e| anyhow!("unexpected response ({status}): {e}"))?;

    if let Some(err) = parsed.errors.first() {
        return Err(anyhow!("{}", err.message));
    }

    let media = parsed
        .data
        .and_then(|data| data.media)
        .ok_or_else(|| anyhow!("empty response ({status})"))?;

    let connection = match kind {
        crate::tui::model::CreditKind::Characters => media.characters,
        crate::tui::model::CreditKind::Staff => media.staff,
    };
    let has_next = connection.page_info.has_next_page;

    Ok((credits(connection), has_next))
}

const STAFF_PAGE: &str = r#"
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
"#;

const CHARACTER_PAGE: &str = r#"
query CharacterPage($id: Int, $page: Int, $perPage: Int) {
  Character(id: $id) {
    id
    name { full }
    image { large }
    description(asHtml: false)
    gender
    age
    bloodType
    favourites
    dateOfBirth { year month day }
    media(page: $page, perPage: $perPage, sort: [START_DATE_DESC]) {
      pageInfo { hasNextPage }
      edges {
        characterRole
        node { id title { romaji english } coverImage { large } format seasonYear }
      }
    }
  }
}
"#;

#[derive(Deserialize)]
struct PersonResponse {
    #[serde(default)]
    data: Option<PersonData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize, Default)]
struct PersonData {
    #[serde(rename = "Staff", default)]
    staff: Option<StaffNode>,
    #[serde(rename = "Character", default)]
    character: Option<CharacterNode>,
}

#[derive(Deserialize)]
struct StaffNode {
    id: i64,
    #[serde(default)]
    name: Option<CreditName>,
    #[serde(default)]
    image: Option<CreditImage>,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "languageV2", default)]
    language: Option<String>,
    #[serde(rename = "homeTown", default)]
    home_town: Option<String>,
    #[serde(rename = "yearsActive", default)]
    years_active: Vec<i64>,
    #[serde(rename = "primaryOccupations", default)]
    occupations: Vec<String>,
    #[serde(default)]
    favourites: Option<i64>,
    #[serde(rename = "dateOfBirth", default)]
    birthday: Option<crate::tui::model::FuzzyDate>,
    #[serde(rename = "staffMedia", default)]
    staff_media: MediaConnection,
    #[serde(rename = "characterMedia", default)]
    character_media: CharacterMediaConnection,
}

#[derive(Deserialize)]
struct CharacterNode {
    id: i64,
    #[serde(default)]
    name: Option<CreditName>,
    #[serde(default)]
    image: Option<CreditImage>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default)]
    age: Option<String>,
    #[serde(rename = "bloodType", default)]
    blood_type: Option<String>,
    #[serde(default)]
    favourites: Option<i64>,
    #[serde(rename = "dateOfBirth", default)]
    birthday: Option<crate::tui::model::FuzzyDate>,
    #[serde(default)]
    media: MediaConnection,
}

#[derive(Deserialize, Default)]
struct MediaConnection {
    #[serde(rename = "pageInfo", default)]
    page_info: crate::tui::model::CreditPageInfo,
    #[serde(default)]
    edges: Vec<MediaEdge>,
}

#[derive(Deserialize)]
struct MediaEdge {
    #[serde(rename = "staffRole", default)]
    staff_role: Option<String>,
    #[serde(rename = "characterRole", default)]
    character_role: Option<String>,
    #[serde(default)]
    node: Option<crate::tui::model::Media>,
}

#[derive(Deserialize, Default)]
struct CharacterMediaConnection {
    #[serde(rename = "pageInfo", default)]
    page_info: crate::tui::model::CreditPageInfo,
    #[serde(default)]
    edges: Vec<CharacterMediaEdge>,
}

#[derive(Deserialize)]
struct CharacterMediaEdge {
    #[serde(default)]
    characters: Vec<NamedNode>,
    #[serde(default)]
    node: Option<crate::tui::model::Media>,
}

#[derive(Deserialize, Default)]
struct NamedNode {
    #[serde(default)]
    name: Option<CreditName>,
}

fn works(connection: MediaConnection, character_roles: bool) -> Vec<crate::tui::model::PersonWork> {
    connection
        .edges
        .into_iter()
        .filter_map(|edge| {
            let role = if character_roles {
                edge.character_role
            } else {
                edge.staff_role
            };
            Some(crate::tui::model::PersonWork {
                role: role.unwrap_or_default(),
                media: edge.node?,
            })
        })
        .collect()
}

/// Staff profile plus one page of their works.
pub async fn media_staff(
    client: &Client,
    cache: CacheRef<'_>,
    id: i64,
    page: u32,
    per_page: u32,
) -> Result<crate::tui::model::PersonPageData> {
    let parsed = post_person(client, cache, STAFF_PAGE, id, page, per_page).await?;
    let staff = parsed.staff.ok_or_else(|| anyhow!("staff not found"))?;

    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(value) = staff.language {
        fields.push(("Language".into(), value));
    }
    if let Some(value) = date_label(&staff.birthday) {
        fields.push(("Birthday".into(), value));
    }
    if let Some(value) = staff.home_town {
        fields.push(("Home Town".into(), value));
    }
    if let Some(value) = years_active(&staff.years_active) {
        fields.push(("Years Active".into(), value));
    }
    if !staff.occupations.is_empty() {
        fields.push(("Occupations".into(), staff.occupations.join(", ")));
    }
    if let Some(value) = staff.favourites {
        fields.push(("Favourites".into(), grouped(value)));
    }

    let anime_roles: Vec<crate::tui::model::PersonWork> = staff
        .staff_media
        .edges
        .into_iter()
        .filter_map(|edge| {
            Some(crate::tui::model::PersonWork {
                role: edge.staff_role.unwrap_or_default(),
                media: edge.node?,
            })
        })
        .collect();
    let va_roles: Vec<crate::tui::model::PersonWork> = staff
        .character_media
        .edges
        .into_iter()
        .filter_map(|edge| {
            Some(crate::tui::model::PersonWork {
                role: edge
                    .characters
                    .into_iter()
                    .filter_map(|node| node.name.and_then(|name| name.full))
                    .collect::<Vec<_>>()
                    .join(", "),
                media: edge.node?,
            })
        })
        .collect();

    Ok(crate::tui::model::PersonPageData {
        kind: crate::tui::model::PersonKind::Staff,
        id: staff.id,
        name: staff
            .name
            .and_then(|name| name.full)
            .unwrap_or_else(|| "?".into()),
        image: staff.image.and_then(|image| image.large),
        description: staff.description,
        fields,
        sections: vec![
            crate::tui::model::PersonSection {
                title: "ANIME STAFF POSITIONS".into(),
                works: anime_roles,
            },
            crate::tui::model::PersonSection {
                title: "VOICE ACTING ROLES".into(),
                works: va_roles,
            },
        ],
        has_next: staff.staff_media.page_info.has_next_page
            || staff.character_media.page_info.has_next_page,
    })
}

/// Character profile plus one page of the media they appear in.
pub async fn media_character(
    client: &Client,
    cache: CacheRef<'_>,
    id: i64,
    page: u32,
    per_page: u32,
) -> Result<crate::tui::model::PersonPageData> {
    let parsed = post_person(client, cache, CHARACTER_PAGE, id, page, per_page).await?;
    let character = parsed
        .character
        .ok_or_else(|| anyhow!("character not found"))?;

    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(value) = character.gender {
        fields.push(("Gender".into(), value));
    }
    if let Some(value) = character.age {
        fields.push(("Age".into(), value));
    }
    if let Some(value) = date_label(&character.birthday) {
        fields.push(("Birthday".into(), value));
    }
    if let Some(value) = character.blood_type {
        fields.push(("Blood Type".into(), value));
    }
    if let Some(value) = character.favourites {
        fields.push(("Favourites".into(), grouped(value)));
    }

    let has_next = character.media.page_info.has_next_page;
    let roles = works(character.media, true);

    Ok(crate::tui::model::PersonPageData {
        kind: crate::tui::model::PersonKind::Character,
        id: character.id,
        name: character
            .name
            .and_then(|name| name.full)
            .unwrap_or_else(|| "?".into()),
        image: character.image.and_then(|image| image.large),
        description: character.description,
        fields,
        sections: vec![crate::tui::model::PersonSection {
            title: "ANIME ROLES".into(),
            works: roles,
        }],
        has_next,
    })
}

async fn post_person(
    client: &Client,
    cache: CacheRef<'_>,
    query: &str,
    id: i64,
    page: u32,
    per_page: u32,
) -> Result<PersonData> {
    let (status, text) = fetch(
        client,
        cache,
        &Body {
            query,
            variables: serde_json::json!({ "id": id, "page": page, "perPage": per_page }),
        },
    )
    .await?;

    let parsed: PersonResponse =
        serde_json::from_str(&text).map_err(|e| anyhow!("unexpected response ({status}): {e}"))?;

    if let Some(err) = parsed.errors.first() {
        return Err(anyhow!("{}", err.message));
    }

    parsed
        .data
        .ok_or_else(|| anyhow!("empty response ({status})"))
}

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

fn years_active(years: &[i64]) -> Option<String> {
    let first = years.iter().min()?;
    let last = years.iter().max()?;
    Some(format!("{first} - {last}"))
}

fn date_label(date: &Option<crate::tui::model::FuzzyDate>) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = date.as_ref()?;
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

pub fn detail_query() -> String {
    // Longest token first: "FIELDS" is a substring of "RELATION_FIELDS".
    DETAIL
        .replace("RELATION_FIELDS", RELATION_FIELDS)
        .replace("FIELDS", MEDIA_FIELDS)
}

#[derive(Deserialize)]
struct DetailResponse {
    #[serde(default)]
    data: Option<DetailData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct DetailData {
    #[serde(rename = "Media", default)]
    media: Option<DetailMedia>,
}

#[derive(Deserialize)]
struct DetailMedia {
    #[serde(flatten)]
    media: Media,
    #[serde(rename = "mediaListEntry", default)]
    list_entry: Option<crate::tui::model::MediaListEntry>,
    #[serde(default)]
    relations: crate::tui::model::Relations,
    #[serde(default)]
    recommendations: crate::tui::model::Recommendations,
    #[serde(default)]
    characters: crate::tui::model::CreditConnection,
    #[serde(default)]
    staff: crate::tui::model::CreditConnection,
}

fn credits(connection: crate::tui::model::CreditConnection) -> Vec<crate::tui::model::Credit> {
    connection
        .edges
        .into_iter()
        .filter_map(|edge| {
            let node = edge.node?;
            Some(crate::tui::model::Credit {
                id: node.id,
                name: node
                    .name
                    .and_then(|name| name.full)
                    .unwrap_or_else(|| "?".into()),
                image: node.image.and_then(|image| image.large),
                role: edge.role,
                voice_actors: edge
                    .voice_actors
                    .into_iter()
                    .filter_map(|voice| voice.name.and_then(|name| name.full))
                    .collect(),
            })
        })
        .collect()
}

pub async fn media_detail(
    client: &Client,
    cache: CacheRef<'_>,
    kind: MediaKind,
    id: i64,
) -> Result<MediaDetail> {
    let (status, text) = fetch(
        client,
        cache,
        &Body {
            query: &detail_query(),
            variables: serde_json::json!({ "id": id, "type": kind.api_name() }),
        },
    )
    .await?;

    let parsed: DetailResponse =
        serde_json::from_str(&text).map_err(|e| anyhow!("unexpected response ({status}): {e}"))?;

    if let Some(err) = parsed.errors.first() {
        return Err(anyhow!("{}", err.message));
    }

    let media = parsed
        .data
        .and_then(|data| data.media)
        .ok_or_else(|| anyhow!("empty response ({status})"))?;

    let mut relations = Vec::new();
    for edge in media.relations.edges {
        if let Some(node) = edge.node {
            relations.push(crate::tui::model::Relation {
                relation_type: edge.relation_type.unwrap_or_default(),
                media: node,
            });
        }
    }

    let recommendations = media
        .recommendations
        .nodes
        .into_iter()
        .filter_map(|node| node.media)
        .collect();

    Ok(MediaDetail {
        media: media.media,
        list_entry: media.list_entry,
        relations,
        recommendations,
        characters: credits(media.characters),
        staff: credits(media.staff),
    })
}

pub async fn media_list(
    client: &Client,
    cache: CacheRef<'_>,
    query: &str,
    variables: Value,
) -> Result<(Vec<Media>, bool)> {
    // AniList reports application errors with a 4xx status and a JSON body, so
    // the body is parsed before the status decides the outcome.
    let (status, text) = fetch(client, cache, &Body { query, variables }).await?;
    let parsed: Response =
        serde_json::from_str(&text).map_err(|e| anyhow!("unexpected response ({status}): {e}"))?;

    if let Some(err) = parsed.errors.first() {
        return Err(anyhow!("{}", err.message));
    }

    let page = parsed
        .data
        .and_then(|d| d.page)
        .ok_or_else(|| anyhow!("empty response ({status})"))?;

    Ok((page.media, page.page_info.has_next_page))
}

#[derive(Deserialize)]
struct MyListResponse {
    #[serde(default)]
    data: Option<MyListData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct MyListData {
    #[serde(rename = "Page", default)]
    page: Option<MyListPage>,
}

#[derive(Deserialize)]
struct MyListPage {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    #[serde(rename = "mediaList", default)]
    entries: Vec<MyListEdge>,
}

#[derive(Deserialize)]
struct MyListEdge {
    #[serde(default)]
    progress: i64,
    #[serde(default)]
    media: Option<Media>,
}

fn my_entries(entries: Vec<MyListEdge>) -> Vec<MyListEntry> {
    entries
        .into_iter()
        .filter_map(|entry| {
            Some(MyListEntry {
                media: entry.media?,
                progress: entry.progress,
            })
        })
        .collect()
}

/// One page of the viewer's current anime or manga list.
pub async fn my_list(
    client: &Client,
    cache: CacheRef<'_>,
    user_id: u64,
    kind: MediaKind,
    page: u32,
    per_page: u32,
) -> Result<(Vec<MyListEntry>, bool)> {
    let (status, text) = fetch(
        client,
        cache,
        &Body {
            query: &my_list_query(),
            variables: serde_json::json!({
                "userId": user_id,
                "type": kind.api_name(),
                "page": page,
                "perPage": per_page,
            }),
        },
    )
    .await?;

    let parsed: MyListResponse =
        serde_json::from_str(&text).map_err(|e| anyhow!("unexpected response ({status}): {e}"))?;

    if let Some(err) = parsed.errors.first() {
        return Err(anyhow!("{}", err.message));
    }

    let page = parsed
        .data
        .and_then(|data| data.page)
        .ok_or_else(|| anyhow!("empty response ({status})"))?;

    Ok((my_entries(page.entries), page.page_info.has_next_page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_payload_maps_credits() {
        let payload = r#"{"data":{"Media":{"id":1,"title":{"romaji":"X"},
          "characters":{"edges":[{"role":"MAIN","node":{"name":{"full":"D"},"image":{"large":"u"}}}]},
          "staff":{"edges":[{"role":"Original Creator","node":{"name":{"full":"Mike"},"image":null}}]}}}}"#;
        let parsed: DetailResponse = serde_json::from_str(payload).expect("parse");
        let media = parsed.data.and_then(|d| d.media).expect("media");
        let characters = credits(media.characters);
        let staff = credits(media.staff);
        assert_eq!(characters.len(), 1);
        assert_eq!(characters[0].name, "D");
        assert_eq!(characters[0].role.as_deref(), Some("MAIN"));
        assert_eq!(staff.len(), 1);
        assert_eq!(staff[0].name, "Mike");
    }

    #[test]
    fn detail_payload_maps_the_viewer_list_entry() {
        let payload = r#"{"data":{"Media":{"id":1,
          "mediaListEntry":{"status":"CURRENT","score":8.5,"progress":5,"repeat":2,"notes":null}}}}"#;
        let parsed: DetailResponse = serde_json::from_str(payload).expect("parse");
        let media = parsed.data.and_then(|d| d.media).expect("media");
        let entry = media.list_entry.expect("list entry");
        assert_eq!(entry.status.as_deref(), Some("CURRENT"));
        assert_eq!(entry.score, Some(8.5));
        assert_eq!(entry.progress, Some(5));
        assert_eq!(entry.repeat, Some(2));
        assert!(entry.notes.is_none());
    }

    #[tokio::test]
    async fn fresh_cache_skips_the_network() {
        let root = std::env::temp_dir().join(format!(
            "aniscrobble-graphql-cache-fresh-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let db = crate::database::Database::open(root.join("data.db")).expect("database");
        let cache = Cache::new(Some(db));
        let body = Body {
            query: "query { Viewer { id } }",
            variables: serde_json::json!({}),
        };
        cache.write("anime:1", br#"{"data":{"Viewer":{"id":1}}}"#);

        let client = Client::new();
        let (status, text) = fetch_from(
            "http://127.0.0.1:1",
            &client,
            Some(CacheMode::LocalFirst(
                &cache,
                "anime:1",
                Duration::from_secs(300),
            )),
            &body,
        )
        .await
        .expect("local hit");

        assert_eq!(status, "cache");
        assert!(text.contains("\"Viewer\""));
        drop(cache);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn my_list_is_network_first_even_with_a_fresh_entry() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;

        let root = std::env::temp_dir().join(format!(
            "aniscrobble-graphql-cache-network-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let db = crate::database::Database::open(root.join("data.db")).expect("database");
        let cache = Cache::new(Some(db));
        let key = "my:1:anime:1";
        cache.write(key, b"cached");

        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(socket.try_clone().expect("clone"));
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).expect("request body");

            let payload = br#"{"data":{"fresh":true}}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    )
                    .as_bytes(),
                )
                .expect("headers");
            socket.write_all(payload).expect("body");
        });

        let body = Body {
            query: "query { Viewer { id } }",
            variables: serde_json::json!({}),
        };
        let client = Client::new();
        let (status, text) = fetch_from(
            &format!("http://{addr}"),
            &client,
            Some(CacheMode::NetworkFirst(&cache, key)),
            &body,
        )
        .await
        .expect("network response");

        assert_eq!(status, "200");
        assert!(text.contains("\"fresh\""), "fresh cache must be bypassed");
        assert_eq!(
            cache.read_any(key),
            Some(br#"{"data":{"fresh":true}}"#.to_vec())
        );
        server.join().expect("server");
        drop(cache);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn expired_cache_falls_back_when_the_network_fails() {
        let root = std::env::temp_dir().join(format!(
            "aniscrobble-graphql-cache-stale-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let db = crate::database::Database::open(root.join("data.db")).expect("database");
        let cache = Cache::new(Some(db));
        let body = Body {
            query: "query { Viewer { id } }",
            variables: serde_json::json!({}),
        };
        let old = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs()
            - 120;
        cache.write_at("anime:1", br#"{"data":{"Viewer":{"id":1}}}"#, old);

        let client = Client::new();
        let (status, text) = fetch_from(
            "http://127.0.0.1:1",
            &client,
            Some(CacheMode::LocalFirst(
                &cache,
                "anime:1",
                Duration::from_secs(60),
            )),
            &body,
        )
        .await
        .expect("stale fallback");

        assert_eq!(status, "cache");
        assert!(text.contains("\"Viewer\""));
        drop(cache);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn uncached_fetch_never_falls_back() {
        let body = Body {
            query: "query Search($search: String) { Page { media(search: $search) { id } } }",
            variables: serde_json::json!({ "search": "x" }),
        };
        let client = Client::new();
        let result = fetch_from("http://127.0.0.1:1", &client, None, &body).await;
        assert!(result.is_err(), "search must not be served from cache");
    }

    #[test]
    fn my_list_payload_maps_progress_and_media_type() {
        let payload = r#"{"data":{"Page":{
          "pageInfo":{"hasNextPage":true},
          "mediaList":[
            {"progress":5,"media":{"id":1,"type":"MANGA","chapters":20}},
            {"progress":2,"media":null}
          ]}}}"#;
        let parsed: MyListResponse = serde_json::from_str(payload).expect("parse");
        let page = parsed.data.and_then(|data| data.page).expect("page");
        assert!(page.page_info.has_next_page);
        let entries = my_entries(page.entries);
        assert_eq!(entries.len(), 1, "entries without media are dropped");
        assert_eq!(entries[0].progress, 5);
        assert_eq!(entries[0].media.kind, MediaKind::Manga);
        assert_eq!(entries[0].media.chapters, Some(20));
    }

    #[test]
    fn credit_page_info_is_read_from_the_graphql_shape() {
        let payload = r#"{"data":{"Media":{"characters":{
          "pageInfo":{"hasNextPage":true},
          "edges":[{"role":"MAIN","voiceActors":[{"name":{"full":"Aoi"}}],
                    "node":{"name":{"full":"D"},"image":null}}]}}}}"#;
        let parsed: CreditsResponse = serde_json::from_str(payload).expect("parse");
        let connection = parsed
            .data
            .and_then(|data| data.media)
            .expect("media")
            .characters;
        assert!(connection.page_info.has_next_page);
        let mapped = credits(connection);
        assert_eq!(mapped[0].voice_actors, vec!["Aoi".to_string()]);
    }
}
