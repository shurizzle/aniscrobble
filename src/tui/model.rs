use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Media {
    pub id: i64,
    #[serde(default)]
    pub title: Title,
    #[serde(rename = "coverImage", default)]
    pub cover_image: CoverImage,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(rename = "seasonYear", default)]
    pub season_year: Option<i64>,
    #[serde(rename = "averageScore", default)]
    pub average_score: Option<i64>,
    #[serde(default)]
    pub episodes: Option<i64>,
    #[serde(default)]
    pub duration: Option<i64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub popularity: Option<i64>,
    #[serde(default)]
    pub favourites: Option<i64>,
    #[serde(default)]
    pub studios: Studios,
    #[serde(default)]
    pub producers: Studios,
    #[serde(default)]
    pub season: Option<String>,
    #[serde(rename = "meanScore", default)]
    pub mean_score: Option<i64>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(rename = "countryOfOrigin", default)]
    pub country_of_origin: Option<String>,
    #[serde(default)]
    pub hashtag: Option<String>,
    #[serde(default)]
    pub synonyms: Vec<String>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(rename = "startDate", default)]
    pub start_date: FuzzyDate,
    #[serde(rename = "endDate", default)]
    pub end_date: FuzzyDate,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

impl Media {
    pub fn title(&self) -> &str {
        self.title
            .english
            .as_deref()
            .or(self.title.romaji.as_deref())
            .unwrap_or("(untitled)")
    }

    pub fn cover_url(&self) -> Option<&str> {
        self.cover_image.large.as_deref()
    }

    /// Higher resolution variant for the media page.
    pub fn cover_url_large(&self) -> Option<&str> {
        self.cover_image
            .extra_large
            .as_deref()
            .or(self.cover_image.large.as_deref())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Title {
    #[serde(default)]
    pub romaji: Option<String>,
    #[serde(default)]
    pub english: Option<String>,
}

/// Full media data plus its relations, recommendations, characters and staff,
/// fetched only when a media page opens.
#[derive(Debug, Clone, Default)]
pub struct MediaDetail {
    pub media: Media,
    /// Viewer's list entry for this media, `None` when logged out or not on the list.
    pub list_entry: Option<MediaListEntry>,
    pub relations: Vec<Relation>,
    pub recommendations: Vec<Media>,
    pub characters: Vec<Credit>,
    pub staff: Vec<Credit>,
}

/// The viewer's AniList list entry for one media.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MediaListEntry {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub progress: Option<i64>,
    #[serde(default)]
    pub repeat: Option<i64>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Which credit list a request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CreditKind {
    Characters,
    Staff,
}

/// Which person page a request targets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PersonKind {
    #[default]
    Staff,
    Character,
}

/// One collaboration on a person page.
#[derive(Debug, Clone, Default)]
pub struct PersonWork {
    pub role: String,
    pub media: Media,
}

/// A titled block of collaborations.
#[derive(Debug, Clone, Default)]
pub struct PersonSection {
    pub title: String,
    pub works: Vec<PersonWork>,
}

/// Staff or character page: profile plus paginated filmography.
#[derive(Debug, Clone, Default)]
pub struct PersonPageData {
    pub kind: PersonKind,
    pub id: i64,
    pub name: String,
    pub image: Option<String>,
    pub description: Option<String>,
    pub fields: Vec<(String, String)>,
    pub sections: Vec<PersonSection>,
    pub has_next: bool,
}

/// A character or a staff member shown on the media page.
#[derive(Debug, Clone, Default)]
pub struct Credit {
    pub id: Option<i64>,
    pub name: String,
    pub image: Option<String>,
    pub role: Option<String>,
    pub voice_actors: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreditNode {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: Option<CreditName>,
    #[serde(default)]
    pub image: Option<CreditImage>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreditName {
    #[serde(default)]
    pub full: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreditImage {
    #[serde(default)]
    pub large: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreditEdge {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(rename = "voiceActors", default)]
    pub voice_actors: Vec<CreditNode>,
    #[serde(default)]
    pub node: Option<CreditNode>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreditConnection {
    #[serde(rename = "pageInfo", default)]
    pub page_info: CreditPageInfo,
    #[serde(default)]
    pub edges: Vec<CreditEdge>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreditPageInfo {
    #[serde(rename = "hasNextPage", default)]
    pub has_next_page: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Relation {
    pub relation_type: String,
    pub media: Media,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RelationEdge {
    #[serde(rename = "relationType", default)]
    pub relation_type: Option<String>,
    #[serde(default)]
    pub node: Option<Media>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Relations {
    #[serde(default)]
    pub edges: Vec<RelationEdge>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RecommendationNode {
    #[serde(rename = "mediaRecommendation", default)]
    pub media: Option<Media>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Recommendations {
    #[serde(default)]
    pub nodes: Vec<RecommendationNode>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Tag {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rank: i64,
    #[serde(rename = "isMediaSpoiler", default)]
    pub is_media_spoiler: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FuzzyDate {
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub month: Option<u32>,
    #[serde(default)]
    pub day: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Studios {
    #[serde(default)]
    pub nodes: Vec<Studio>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Studio {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CoverImage {
    #[serde(default)]
    pub large: Option<String>,
    #[serde(rename = "extraLarge", default)]
    pub extra_large: Option<String>,
}
