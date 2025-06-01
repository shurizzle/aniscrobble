use std::ops::Deref;

use anyhow::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use ureq::RequestBuilder;

pub struct Api(ureq::Agent);

struct Query(Box<[u8]>);

impl std::fmt::Debug for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Query")
            .field(&unsafe { core::str::from_utf8_unchecked(&self.0) })
            .finish()
    }
}

impl AsRef<Query> for Query {
    #[inline(always)]
    fn as_ref(&self) -> &Query {
        self
    }
}

struct QueryBuilder(Vec<u8>);

impl QueryBuilder {
    pub fn new(query: impl AsRef<str>) -> Self {
        let mut buf = Vec::new();
        buf.extend_from_slice("{\"query\":".as_bytes());
        _ = serde_json::to_writer(&mut buf, query.as_ref());
        buf.extend_from_slice(",\"variables\":{".as_bytes());
        Self(buf)
    }

    pub fn push<T: Serialize>(&mut self, name: impl AsRef<str>, v: &T) -> Result<(), ureq::Error> {
        if self.0[self.0.len() - 1] != b'{' {
            self.0.push(b',');
        }
        serde_json::to_writer(&mut self.0, name.as_ref())?;
        self.0.push(b':');
        serde_json::to_writer(&mut self.0, v)?;
        Ok(())
    }

    pub fn add<T: Serialize>(mut self, name: impl AsRef<str>, v: &T) -> Result<Self, ureq::Error> {
        self.push(name, v)?;
        Ok(self)
    }

    pub fn build(mut self) -> Query {
        self.0.extend_from_slice("}}".as_bytes());
        let Self(buf) = self;
        Query(buf.into_boxed_slice())
    }
}

impl From<QueryBuilder> for Query {
    #[inline(always)]
    fn from(value: QueryBuilder) -> Self {
        value.build()
    }
}

impl Deref for Query {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MediaListStatus {
    Current,
    Planning,
    Completed,
    Dropped,
    Paused,
    Repeating,
}

#[derive(Debug)]
pub struct Anime {
    pub episodes: u64,
    pub progress: u64,
}

impl Api {
    pub fn new() -> Self {
        Self(ureq::Agent::new_with_defaults())
    }

    fn request<T: DeserializeOwned>(
        &self,
        token: Option<&str>,
        query: impl AsRef<Query>,
    ) -> Result<T, ureq::Error> {
        fn put_auth<T>(mut builder: RequestBuilder<T>, token: Option<&str>) -> RequestBuilder<T> {
            if let Some(token) = token {
                builder = builder.header("Authorization", format!("Bearer {token}"));
            }
            builder
        }

        Ok(put_auth(
            self.0
                .post("https://graphql.anilist.co")
                .header("Accept", "application/json")
                .header("Content-Type", "application/json"),
            token,
        )
        .send(&**query.as_ref())?
        .into_body()
        .read_json::<Data<T>>()?
        .data)
    }

    pub fn me(&self, token: &str) -> Result<u64, ureq::Error> {
        #[derive(Deserialize)]
        struct Viewer {
            id: u64,
        }

        #[derive(Deserialize)]
        #[allow(non_snake_case)]
        struct Container {
            Viewer: Viewer,
        }

        self.request::<Container>(
            Some(token),
            QueryBuilder::new("query { Viewer { id } }").build(),
        )
        .map(|v| v.Viewer.id)
    }

    fn _get_anime(&self, id: u64) -> Result<u64, ureq::Error> {
        #[derive(Deserialize)]
        struct Media {
            episodes: u64,
        }

        #[derive(Deserialize)]
        #[allow(non_snake_case)]
        struct Container {
            Media: Media,
        }

        const QUERY: &str = "
        query ($id: Int) {
            Media(id: $id, type: ANIME) {
                episodes
            }
        }
        ";

        self.request::<Container>(None, QueryBuilder::new(QUERY).add("id", &id)?.build())
            .map(|p| p.Media.episodes)
    }

    fn _get_progess(&self, token: &str, user_id: u64, id: u64) -> Result<u64, ureq::Error> {
        #[derive(Deserialize)]
        struct MediaList {
            progress: u64,
        }

        #[derive(Deserialize)]
        #[allow(non_snake_case)]
        struct Container {
            MediaList: MediaList,
        }

        const QUERY: &str = "
        query ($userId: Int, $mediaId: Int) {
            MediaList(userId: $userId, mediaId: $mediaId, type: ANIME) {
                progress
            }
        }
        ";

        self.request::<Container>(
            Some(token),
            QueryBuilder::new(QUERY)
                .add("userId", &user_id)?
                .add("mediaId", &id)?
                .build(),
        )
        .map(|p| p.MediaList.progress)
    }

    pub fn get_progress(&self, token: &str, user_id: u64, id: u64) -> Result<Anime, ureq::Error> {
        let episodes = self._get_anime(id)?;
        let progress = match self._get_progess(token, user_id, id) {
            Ok(p) => p,
            Err(ureq::Error::StatusCode(404)) => 0,
            Err(err) => return Err(err),
        };
        Ok(Anime { episodes, progress })
    }

    pub fn set_progress(
        &self,
        token: &str,
        id: u64,
        progress: u64,
        total: u64,
    ) -> Result<u64, ureq::Error> {
        #[derive(Deserialize)]
        struct SaveMediaListEntry {
            progress: u64,
        }

        #[derive(Deserialize)]
        #[allow(non_snake_case)]
        struct Container {
            SaveMediaListEntry: SaveMediaListEntry,
        }

        const QUERY: &str = "
        mutation ($mediaId: Int, $status: MediaListStatus, $progress: Int) {
            SaveMediaListEntry (mediaId: $mediaId, status: $status, progress: $progress) {
                progress
            }
        }
        ";
        self.request::<Container>(
            Some(token),
            QueryBuilder::new(QUERY)
                .add("mediaId", &id)?
                .add("progress", &progress)?
                .add(
                    "status",
                    &if progress == total {
                        MediaListStatus::Completed
                    } else {
                        MediaListStatus::Current
                    },
                )?
                .build(),
        )
        .map(|p| p.SaveMediaListEntry.progress)
    }
}
