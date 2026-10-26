//! Persistent cache for everything the TUI fetches, stored in the same LMDB
//! environment as progress and login.
//!
//! Keys are semantic: `anime:<id>`, `anime:<id>:cover`, `staff:<id>:<page>`.
//! Each entry carries the epoch second it was written. Fresh entries are served
//! locally, expired ones are fetched again and kept only as an offline fallback.
//! Search is never cached.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::database::Database;

const MAGIC: &[u8; 3] = b"ct1";
const HEADER: usize = 3 + 8;

#[derive(Debug, Clone)]
pub struct Cache {
    db: Option<Database>,
}

impl Cache {
    pub fn new(db: Option<Database>) -> Self {
        Self { db }
    }

    /// Cached payload when it is at most `ttl` old.
    pub fn read_fresh(&self, key: &str, ttl: Duration) -> Option<Vec<u8>> {
        let entry = self.load(key)?;
        (now().saturating_sub(entry.at) <= ttl.as_secs()).then_some(entry.value)
    }

    /// Cached payload regardless of age, for fallback when the network fails.
    pub fn read_any(&self, key: &str) -> Option<Vec<u8>> {
        self.load(key).map(|entry| entry.value)
    }

    pub fn write(&self, key: &str, bytes: &[u8]) {
        self.store(key, bytes, now());
    }

    #[cfg(test)]
    pub(crate) fn write_at(&self, key: &str, bytes: &[u8], at: u64) {
        self.store(key, bytes, at);
    }

    fn store(&self, key: &str, bytes: &[u8], at: u64) {
        let Some(db) = &self.db else {
            return;
        };
        let mut value = Vec::with_capacity(HEADER + bytes.len());
        value.extend_from_slice(MAGIC);
        value.extend_from_slice(&at.to_le_bytes());
        value.extend_from_slice(bytes);
        let _ = db.cache_put(key, &value);
    }

    fn load(&self, key: &str) -> Option<Entry> {
        let raw = self.db.as_ref()?.cache_get(key).ok().flatten()?;
        Some(decode(raw))
    }
}

struct Entry {
    at: u64,
    value: Vec<u8>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| age.as_secs())
}

fn decode(raw: Vec<u8>) -> Entry {
    if raw.len() >= HEADER && &raw[..MAGIC.len()] == MAGIC {
        let at = u64::from_le_bytes(raw[MAGIC.len()..HEADER].try_into().expect("8 bytes"));
        Entry {
            at,
            value: raw[HEADER..].to_vec(),
        }
    } else {
        // Entries written before timestamps existed are always stale but still
        // usable as an offline fallback.
        Entry { at: 0, value: raw }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("aniscrobble-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn cache_at(root: &std::path::Path) -> Cache {
        let db = crate::database::Database::open(root.join("data.db")).expect("database");
        Cache::new(Some(db))
    }

    #[test]
    fn cache_without_a_database_is_a_no_op() {
        let cache = Cache::new(None);
        cache.write("anime:1", b"x");
        assert_eq!(cache.read_any("anime:1"), None);
        assert_eq!(cache.read_fresh("anime:1", Duration::from_secs(60)), None);
    }

    #[test]
    fn fresh_entries_are_local_and_expired_ones_are_stale() {
        let root = temp_root("ttl");
        let cache = cache_at(&root);
        cache.write_at("anime:1", b"payload", now() - 120);

        assert_eq!(cache.read_fresh("anime:1", Duration::from_secs(60)), None);
        assert_eq!(
            cache.read_fresh("anime:1", Duration::from_secs(300)),
            Some(b"payload".to_vec())
        );
        assert_eq!(cache.read_any("anime:1"), Some(b"payload".to_vec()));

        drop(cache);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_entries_are_stale_but_readable() {
        let entry = decode(br#"{"data":{}}"#.to_vec());
        assert_eq!(entry.at, 0);
        assert_eq!(entry.value, br#"{"data":{}}"#);
    }
}
