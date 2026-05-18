//! Pivot-parse LRU cache for the Predict surface (P5 KG-02).
//!
//! `indexer::predict_next_actions` re-parses each neighbor session JSONL on
//! demand (this is the deliberate "payload stays lean, Replay re-parses on
//! demand" architecture from `docs/architecture.md`). When a user explores
//! several active sessions in a row, the same set of nearest-neighbour
//! sessions tends to re-appear; without a cache we re-parse a 5 MB JSONL on
//! each call for ~50 ms.
//!
//! This LRU caps at `CAPACITY=64` entries keyed by `(path, mtime)` so a file
//! that gets re-saved (mtime advances) becomes a cache miss and re-parses
//! cleanly. The stored value is `Arc<Session>` so concurrent callers share
//! the same allocation.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Result;
use lru::LruCache;

use crate::parser::Session;

/// 64 entries × ~5 KB per Session (post-parse, mostly text) ≈ 320 KB worst
/// case. Tiny.
const CAPACITY: usize = 64;

pub type CacheKey = (PathBuf, SystemTime);

pub struct ParseLruCache {
    inner: Mutex<LruCache<CacheKey, Arc<Session>>>,
}

impl Default for ParseLruCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseLruCache {
    pub fn new() -> Self {
        // SAFETY: CAPACITY is a non-zero const.
        let cap = NonZeroUsize::new(CAPACITY).expect("CAPACITY > 0");
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Get the parsed Session by (path, mtime). On miss, invoke `parse_fn`,
    /// store, and return. The closure is called outside the lock so concurrent
    /// callers don't serialize on parsing.
    pub fn get_or_parse<F>(&self, path: PathBuf, mtime: SystemTime, parse_fn: F) -> Result<Arc<Session>>
    where
        F: FnOnce(&std::path::Path) -> Result<Session>,
    {
        let key = (path.clone(), mtime);
        // Fast path.
        if let Ok(mut g) = self.inner.lock() {
            if let Some(s) = g.get(&key) {
                return Ok(s.clone());
            }
        }
        // Slow path — parse, then store.
        let parsed = Arc::new(parse_fn(&path)?);
        if let Ok(mut g) = self.inner.lock() {
            g.put(key, parsed.clone());
        }
        Ok(parsed)
    }

    /// Live entry count — for tests/debug only.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// `true` if the cache currently has no entries.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn dummy_session(id: &str) -> Session {
        Session {
            session_id: id.to_string(),
            source_path: "/tmp/x.jsonl".into(),
            project_path: None,
            project_name: None,
            git_branch: None,
            claude_version: None,
            ai_title: None,
            start_time: None,
            end_time: None,
            turns: Vec::new(),
            event_counts: crate::parser::EventCounts::default(),
        }
    }

    #[test]
    fn t_lru_hit_avoids_reparse() {
        let cache = ParseLruCache::new();
        let calls = AtomicUsize::new(0);
        let path = PathBuf::from("/tmp/a.jsonl");
        let mt = SystemTime::UNIX_EPOCH;
        let _ = cache
            .get_or_parse(path.clone(), mt, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_session("a"))
            })
            .unwrap();
        let _ = cache
            .get_or_parse(path, mt, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_session("a"))
            })
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn t_lru_miss_on_mtime_advance() {
        let cache = ParseLruCache::new();
        let calls = AtomicUsize::new(0);
        let path = PathBuf::from("/tmp/b.jsonl");
        let _ = cache
            .get_or_parse(path.clone(), SystemTime::UNIX_EPOCH, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_session("b"))
            })
            .unwrap();
        // mtime advances → fresh parse required.
        let later = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60);
        let _ = cache
            .get_or_parse(path, later, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_session("b"))
            })
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn t_lru_capacity_enforced() {
        let cache = ParseLruCache::new();
        for i in 0..(CAPACITY + 16) {
            let path = PathBuf::from(format!("/tmp/{i}.jsonl"));
            let _ = cache
                .get_or_parse(path, SystemTime::UNIX_EPOCH, |_| Ok(dummy_session(&i.to_string())))
                .unwrap();
        }
        assert!(cache.len() <= CAPACITY);
    }

    #[test]
    fn t_lru_returns_arc_clone() {
        // Two calls return Arc clones that share the underlying allocation.
        let cache = ParseLruCache::new();
        let path = PathBuf::from("/tmp/c.jsonl");
        let mt = SystemTime::UNIX_EPOCH;
        let a = cache
            .get_or_parse(path.clone(), mt, |_| Ok(dummy_session("c")))
            .unwrap();
        let b = cache
            .get_or_parse(path, mt, |_| Ok(dummy_session("c")))
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b), "two hits should return the same Arc");
    }
}
