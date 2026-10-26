//! Cancellable request spawning.
//!
//! A [`ScopeId`] is one mounted view. Each request belongs to a scope and to a
//! [`ReqKey`]. Retiring a scope cancels every request under it; issuing the same
//! key again cancels only the previous request for that key.
//!
//! Every message carries `(scope, req_id)` so the UI can drop results that were
//! already abandoned, even when an abort loses the race against a response that
//! is already in the channel.

pub mod graphql;
pub mod rate;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use serde_json::json;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use crate::tui::model::{Credit, CreditKind, Media, MediaDetail, PersonKind, PersonPageData};
use rate::RateLimiter;

pub type ScopeId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReqKey {
    Trending(u32),
    Search(String, u32),
    Detail(i64),
    Browse(String, u32),
    Credits(CreditKind, u32),
    Person(PersonKind, i64, u32),
    Cover { url: String, w: u16, h: u16 },
}

#[derive(Debug, Clone)]
pub enum Job {
    Trending {
        page: u32,
        per_page: u32,
    },
    Search {
        query: String,
        page: u32,
        per_page: u32,
    },
    /// Genre or tag browse, resolved server side.
    Browse {
        genre: Option<String>,
        tag: Option<String>,
        page: u32,
        per_page: u32,
    },
    /// One page of the full character or staff list.
    Credits {
        kind: CreditKind,
        id: i64,
        page: u32,
        per_page: u32,
    },
    /// Staff or character profile and one page of their works.
    Person {
        kind: PersonKind,
        id: i64,
        page: u32,
        per_page: u32,
    },
    /// Extra data for the media page: relations and recommendations.
    Detail {
        id: i64,
    },
    Cover {
        url: String,
        w: u16,
        h: u16,
    },
}

pub enum NetMsg {
    List {
        scope: ScopeId,
        req: u64,
        page: u32,
        items: Vec<Media>,
        has_next: bool,
    },
    Detail {
        scope: ScopeId,
        req: u64,
        data: Box<MediaDetail>,
    },
    Person {
        scope: ScopeId,
        req: u64,
        page: u32,
        data: Box<PersonPageData>,
    },
    Credits {
        scope: ScopeId,
        req: u64,
        kind: CreditKind,
        page: u32,
        items: Vec<Credit>,
        has_next: bool,
    },
    /// Content addressed: a cover stays valid even when its scope is gone, so
    /// it is cached even if `scope` has already been retired.
    Cover {
        scope: ScopeId,
        req: u64,
        url: String,
        w: u16,
        h: u16,
        proto: Arc<Protocol>,
    },
    Failed {
        scope: ScopeId,
        req: u64,
        key: ReqKey,
        err: String,
    },
}

/// Shared, immutable dependencies of every worker task.
struct Runner {
    /// GraphQL client. Carries the stored login token when there is one.
    client: reqwest::Client,
    /// Image client. Never sends the token to the AniList CDN.
    images: reqwest::Client,
    picker: Arc<Picker>,
    gql: Semaphore,
    covers: Semaphore,
    rate: RateLimiter,
    tx: mpsc::Sender<NetMsg>,
}

pub struct Net {
    runner: Arc<Runner>,
    scopes: HashMap<ScopeId, CancellationToken>,
    current: HashMap<(ScopeId, ReqKey), u64>,
    aborts: HashMap<(ScopeId, u64), AbortHandle>,
    next_scope: ScopeId,
    next_req: u64,
}

impl Net {
    #[cfg(test)]
    pub fn new(picker: Arc<Picker>, tx: mpsc::Sender<NetMsg>) -> Self {
        Self::with_token(picker, tx, None)
    }

    pub fn with_token(
        picker: Arc<Picker>,
        tx: mpsc::Sender<NetMsg>,
        token: Option<String>,
    ) -> Self {
        let build = || {
            reqwest::Client::builder()
                .user_agent(graphql::USER_AGENT)
                .build()
                .expect("reqwest client")
        };
        let images = build();
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = token.as_deref()
            && let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        let client = reqwest::Client::builder()
            .user_agent(graphql::USER_AGENT)
            .default_headers(headers)
            .build()
            .expect("reqwest client");

        Self {
            runner: Arc::new(Runner {
                client,
                images,
                picker,
                gql: Semaphore::new(4),
                covers: Semaphore::new(3),
                rate: RateLimiter::new(10.0, std::time::Duration::from_millis(600)),
                tx,
            }),
            scopes: HashMap::new(),
            current: HashMap::new(),
            aborts: HashMap::new(),
            next_scope: 0,
            next_req: 0,
        }
    }

    /// Mount a new widget scope. The caller owns retirement.
    pub fn mount(&mut self) -> ScopeId {
        self.next_scope += 1;
        self.scopes
            .insert(self.next_scope, CancellationToken::new());
        self.next_scope
    }

    /// Unmount a scope: cancel and abort everything it started.
    pub fn retire(&mut self, scope: ScopeId) {
        if let Some(token) = self.scopes.remove(&scope) {
            token.cancel();
        }
        let stale: Vec<(ScopeId, u64)> = self
            .aborts
            .keys()
            .filter(|(s, _)| *s == scope)
            .cloned()
            .collect();
        for key in stale {
            if let Some(abort) = self.aborts.remove(&key) {
                abort.abort();
            }
        }
        self.current.retain(|(s, _), _| *s != scope);
    }

    /// Mark a request as settled. Called for every received message so the
    /// in-flight counter reflects reality instead of only cancellations.
    pub fn done(&mut self, scope: ScopeId, req: u64) {
        self.aborts.remove(&(scope, req));
    }

    pub fn is_live(&self, scope: ScopeId) -> bool {
        self.scopes.contains_key(&scope)
    }

    pub fn is_current(&self, scope: ScopeId, req: u64) -> bool {
        self.current
            .iter()
            .any(|((s, _), r)| *s == scope && *r == req)
    }

    #[cfg(test)]
    pub fn last_req_for_test(&self, scope: ScopeId) -> Option<u64> {
        self.current
            .iter()
            .filter(|((s, _), _)| *s == scope)
            .map(|(_, req)| *req)
            .max()
    }

    pub fn inflight(&self) -> usize {
        self.aborts.len()
    }

    /// Register a request, cancelling any previous request with the same key.
    /// Returns `None` when the scope is already retired.
    fn register(&mut self, scope: ScopeId, key: &ReqKey) -> Option<(u64, CancellationToken)> {
        let token = self.scopes.get(&scope)?.child_token();

        self.next_req += 1;
        let req = self.next_req;

        if let Some(previous) = self.current.insert((scope, key.clone()), req)
            && let Some(abort) = self.aborts.remove(&(scope, previous))
        {
            abort.abort();
        }

        Some((req, token))
    }

    /// Spawn a request inside a live scope. Returns false when the scope is
    /// gone, in which case nothing is started.
    pub fn request(&mut self, scope: ScopeId, key: ReqKey, job: Job) -> bool {
        let Some((req, token)) = self.register(scope, &key) else {
            return false;
        };

        let runner = self.runner.clone();
        let tx = runner.tx.clone();
        let fail_key = key.clone();

        let task = tokio::spawn(async move {
            let work = run(&runner, scope, req, job);
            let msg = tokio::select! {
                biased;
                _ = token.cancelled() => return,
                msg = work => msg,
            };

            let msg = match msg {
                Ok(msg) => msg,
                Err(err) => NetMsg::Failed {
                    scope,
                    req,
                    key: fail_key,
                    err: format!("{err:#}"),
                },
            };
            let _ = tx.send(msg).await;
        });

        self.aborts.insert((scope, req), task.abort_handle());
        true
    }
}

async fn run(runner: &Runner, scope: ScopeId, req: u64, job: Job) -> Result<NetMsg> {
    match job {
        Job::Trending { page, per_page } => {
            let items = {
                let _permit = runner.gql.acquire().await?;
                runner.rate.acquire().await;
                graphql::media_list(
                    &runner.client,
                    &graphql::trending_query(),
                    json!({ "page": page, "perPage": per_page }),
                )
                .await?
            };
            Ok(NetMsg::List {
                scope,
                req,
                page,
                items: items.0,
                has_next: items.1,
            })
        }
        Job::Search {
            query,
            page,
            per_page,
        } => {
            let items = {
                let _permit = runner.gql.acquire().await?;
                runner.rate.acquire().await;
                graphql::media_list(
                    &runner.client,
                    &graphql::search_query(),
                    json!({ "search": query, "page": page, "perPage": per_page }),
                )
                .await?
            };
            Ok(NetMsg::List {
                scope,
                req,
                page,
                items: items.0,
                has_next: items.1,
            })
        }
        Job::Browse {
            genre,
            tag,
            page,
            per_page,
        } => {
            let items = {
                let _permit = runner.gql.acquire().await?;
                runner.rate.acquire().await;
                graphql::media_list(
                    &runner.client,
                    &graphql::browse_query(),
                    json!({ "genre": genre, "tag": tag, "page": page, "perPage": per_page }),
                )
                .await?
            };
            Ok(NetMsg::List {
                scope,
                req,
                page,
                items: items.0,
                has_next: items.1,
            })
        }
        Job::Credits {
            kind,
            id,
            page,
            per_page,
        } => {
            let (items, has_next) = {
                let _permit = runner.gql.acquire().await?;
                runner.rate.acquire().await;
                graphql::media_credits(&runner.client, kind, id, page, per_page).await?
            };
            Ok(NetMsg::Credits {
                scope,
                req,
                kind,
                page,
                items,
                has_next,
            })
        }
        Job::Person {
            kind,
            id,
            page,
            per_page,
        } => {
            let data = {
                let _permit = runner.gql.acquire().await?;
                runner.rate.acquire().await;
                match kind {
                    PersonKind::Staff => {
                        graphql::media_staff(&runner.client, id, page, per_page).await?
                    }
                    PersonKind::Character => {
                        graphql::media_character(&runner.client, id, page, per_page).await?
                    }
                }
            };
            Ok(NetMsg::Person {
                scope,
                req,
                page,
                data: Box::new(data),
            })
        }
        Job::Detail { id } => {
            let data = {
                let _permit = runner.gql.acquire().await?;
                runner.rate.acquire().await;
                graphql::media_detail(&runner.client, id).await?
            };
            Ok(NetMsg::Detail {
                scope,
                req,
                data: Box::new(data),
            })
        }
        Job::Cover { url, w, h } => {
            let bytes = {
                let _permit = runner.covers.acquire().await?;
                runner
                    .images
                    .get(&url)
                    .header(reqwest::header::USER_AGENT, graphql::USER_AGENT)
                    .send()
                    .await?
                    .error_for_status()?
                    .bytes()
                    .await?
            };

            let picker = runner.picker.clone();
            // Decoding and encoding are CPU bound: never on the UI task.
            let proto = tokio::task::spawn_blocking(move || -> Result<Protocol> {
                let image = image::load_from_memory(&bytes)?;
                Ok(picker.new_protocol(image, Size::new(w, h), Resize::Fit(None))?)
            })
            .await??;

            Ok(NetMsg::Cover {
                scope,
                req,
                url,
                w,
                h,
                proto: Arc::new(proto),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc;

    use super::{Job, Net, NetMsg, ReqKey};

    fn net() -> Net {
        let (tx, _rx) = mpsc::channel(4);
        Net::new(Arc::new(Picker::halfblocks()), tx)
    }

    #[test]
    fn request_on_dead_scope_is_ignored() {
        let mut net = net();
        let scope = net.mount();
        net.retire(scope);

        let key = ReqKey::Trending(1);
        assert!(net.register(scope, &key).is_none());
        assert!(!net.is_live(scope));
    }

    #[test]
    fn same_key_supersedes_but_different_keys_coexist() {
        let mut net = net();
        let scope = net.mount();
        let page = ReqKey::Trending(1);
        let cover = ReqKey::Cover {
            url: "https://example.test/a.jpg".into(),
            w: 10,
            h: 6,
        };

        let (first, _) = net.register(scope, &page).unwrap();
        let (second, _) = net.register(scope, &page).unwrap();
        let (cover_req, _) = net.register(scope, &cover).unwrap();

        assert_ne!(first, second);
        assert!(!net.is_current(scope, first));
        assert!(net.is_current(scope, second));
        assert!(net.is_current(scope, cover_req));
    }

    #[tokio::test]
    async fn settled_request_leaves_the_inflight_counter() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut net = Net::new(Arc::new(Picker::halfblocks()), tx);
        let scope = net.mount();
        let url = "http://127.0.0.1:1/none.png".to_string();

        // Connection refused: the job fails fast without touching the network
        // outside localhost.
        assert!(net.request(
            scope,
            ReqKey::Cover {
                url: url.clone(),
                w: 2,
                h: 2
            },
            Job::Cover {
                url: url.clone(),
                w: 2,
                h: 2
            },
        ));
        assert_eq!(net.inflight(), 1);

        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("job did not settle")
            .expect("channel closed");
        match msg {
            NetMsg::Failed { scope, req, .. } => net.done(scope, req),
            _ => panic!("expected a failure"),
        }

        assert_eq!(net.inflight(), 0);
    }

    #[test]
    fn retire_cancels_every_child_request() {
        let mut net = net();
        let scope = net.mount();
        let page = ReqKey::Trending(1);
        let cover = ReqKey::Cover {
            url: "https://example.test/a.jpg".into(),
            w: 10,
            h: 6,
        };

        let (_, page_token) = net.register(scope, &page).unwrap();
        let (_, cover_token) = net.register(scope, &cover).unwrap();

        net.retire(scope);

        assert!(page_token.is_cancelled());
        assert!(cover_token.is_cancelled());
        assert!(!net.is_current(scope, 1));
        assert!(net.register(scope, &page).is_none());
    }
}
