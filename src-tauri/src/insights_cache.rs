//! Topology insights memoization cache (P5 KG-01).
//!
//! `indexer::topology` calls `compute_insights` which walks every JSONL file
//! under `~/.claude/projects` (and `~/.codex/sessions`). For an 80-session
//! corpus that costs ~80-200 ms each time the user touches the topology view.
//! This cache memoizes the result keyed by `(root_path, max_mtime_seen)` so
//! repeated topology renders hit O(1) once the corpus is steady.
//!
//! Eviction is LRU-style with `MAX_ENTRIES=16` — generous enough to cover
//! both roots × a few mtime snapshots, small enough that the cache itself
//! never costs measurable RAM.
//!
//! The cache stores **owned** results behind `Arc` so concurrent topology
//! requests share the same allocation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Result;
use walkdir::WalkDir;

use crate::indexer::{GapInsight, ProjectInsight};

/// Generous upper bound; in practice we expect at most 2-4 live entries
/// (one per sandbox root × current mtime).
const MAX_ENTRIES: usize = 16;

/// Cache key — (canonical root path, max mtime under that root).
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct CacheKey {
    pub root: PathBuf,
    pub max_mtime: SystemTime,
}

/// The shape `topology()` actually wants back from the heavy walk.
#[derive(Debug, Clone)]
pub struct CachedInsights {
    pub project_insights: Vec<ProjectInsight>,
    pub gap_insights: Vec<GapInsight>,
}

/// LRU-style cache. Insertions newer than `MAX_ENTRIES` evict the
/// least-recently-inserted entry.
#[derive(Default)]
pub struct InsightsCache {
    inner: Mutex<InsightsCacheInner>,
}

#[derive(Default)]
struct InsightsCacheInner {
    map: HashMap<CacheKey, (u64, Arc<CachedInsights>)>,
    counter: u64,
}

impl InsightsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the max-mtime fingerprint for a root by walking `*.jsonl`
    /// files. Returns `UNIX_EPOCH` if no jsonl files are present.
    pub fn fingerprint(root: &std::path::Path) -> SystemTime {
        let mut max_mt = SystemTime::UNIX_EPOCH;
        for entry in WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(mt) = meta.modified() {
                    if mt > max_mt {
                        max_mt = mt;
                    }
                }
            }
        }
        max_mt
    }

    /// Get the cached value, or compute + store + return it.
    pub fn get_or_compute<F>(&self, root: PathBuf, max_mtime: SystemTime, compute: F) -> Result<Arc<CachedInsights>>
    where
        F: FnOnce() -> Result<CachedInsights>,
    {
        let key = CacheKey { root, max_mtime };
        // Fast path — read lock.
        {
            let mut guard = self.inner.lock().map_err(|_| anyhow::anyhow!("insights_cache mutex poisoned"))?;
            let next_tick = guard.counter.saturating_add(1);
            if let Some(entry) = guard.map.get_mut(&key) {
                entry.0 = next_tick;
                let cloned = entry.1.clone();
                guard.counter = next_tick;
                return Ok(cloned);
            }
        }
        // Slow path — compute outside the lock so the work is parallel-safe.
        let computed = Arc::new(compute()?);
        {
            let mut guard = self.inner.lock().map_err(|_| anyhow::anyhow!("insights_cache mutex poisoned"))?;
            guard.counter = guard.counter.saturating_add(1);
            let tick = guard.counter;
            guard.map.insert(key, (tick, computed.clone()));
            // Evict LRU until under cap.
            while guard.map.len() > MAX_ENTRIES {
                if let Some(victim) = guard
                    .map
                    .iter()
                    .min_by_key(|(_, (t, _))| *t)
                    .map(|(k, _)| k.clone())
                {
                    guard.map.remove(&victim);
                } else {
                    break;
                }
            }
            Ok(computed)
        }
    }

    /// Number of live entries — for tests and debug.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|g| g.map.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    fn dummy_insights() -> CachedInsights {
        CachedInsights {
            project_insights: Vec::new(),
            gap_insights: Vec::new(),
        }
    }

    #[test]
    fn t_cache_hit_avoids_recompute() {
        let cache = InsightsCache::new();
        let calls = AtomicUsize::new(0);
        let key_root = PathBuf::from("/tmp/x");
        let key_mt = SystemTime::UNIX_EPOCH;

        let _ = cache
            .get_or_compute(key_root.clone(), key_mt, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_insights())
            })
            .unwrap();
        let _ = cache
            .get_or_compute(key_root, key_mt, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_insights())
            })
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "second call must hit cache");
    }

    #[test]
    fn t_cache_miss_on_mtime_change() {
        let cache = InsightsCache::new();
        let calls = AtomicUsize::new(0);
        let root = PathBuf::from("/tmp/y");

        let _ = cache
            .get_or_compute(root.clone(), SystemTime::UNIX_EPOCH, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_insights())
            })
            .unwrap();
        // Different mtime → different key.
        let later = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60);
        let _ = cache
            .get_or_compute(root, later, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(dummy_insights())
            })
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn t_cache_lru_eviction() {
        let cache = InsightsCache::new();
        // Insert MAX_ENTRIES+5 entries with unique keys.
        for i in 0..(MAX_ENTRIES + 5) {
            let root = PathBuf::from(format!("/tmp/r{i}"));
            let _ = cache
                .get_or_compute(root, SystemTime::UNIX_EPOCH, || Ok(dummy_insights()))
                .unwrap();
        }
        assert!(
            cache.len() <= MAX_ENTRIES,
            "cache must not exceed cap, was {}",
            cache.len()
        );
    }

    #[test]
    fn t_fingerprint_picks_max_mtime() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("a.jsonl");
        let b = td.path().join("b.jsonl");
        fs::write(&a, "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&b, "{}").unwrap();
        let fp = InsightsCache::fingerprint(td.path());
        let b_mt = fs::metadata(&b).unwrap().modified().unwrap();
        let a_mt = fs::metadata(&a).unwrap().modified().unwrap();
        // fp must be >= max(a_mt, b_mt) (could equal one of them).
        let expected_max = if a_mt > b_mt { a_mt } else { b_mt };
        assert_eq!(fp, expected_max);
    }
}
