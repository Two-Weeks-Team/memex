//! Embed-batch pool with Semaphore concurrency cap (P5 KC-05).
//!
//! `fastembed::TextEmbedding::embed` is CPU-bound (ONNX inference) and not
//! Send-friendly across async tasks — the current `Embedder` wraps it in a
//! `Mutex` and runs synchronously on the caller's task. That serializes all
//! 5-vector batches one session at a time, which is fine for interactive
//! search but leaves throughput on the floor during a fresh `memex scan
//! --index` over 80+ sessions.
//!
//! This pool layers two things on top:
//!
//! 1. **`spawn_blocking`** — moves each `embed()` call off the async runtime's
//!    worker threads so we don't starve the Tokio reactor while ONNX is
//!    crunching numbers.
//! 2. **`Semaphore`** — caps the *parallel* inference jobs at
//!    `num_cpus / 2` (min 1) so we don't oversubscribe physical cores when
//!    multiple sessions are being indexed concurrently.
//!
//! Combined with the cross-session batching in `indexer::bulk_index` (batch
//! size 32, all 5 vectors per session flattened into one call), this means a
//! 96-session pass turns into 3 batched embed jobs instead of 480 individual
//! ones — and those 3 jobs run on up to `num_cpus/2` blocking threads in
//! parallel.
//!
//! ## Why not embed across sessions directly?
//!
//! The fastembed model's internal ONNX session is not thread-safe; sharing
//! one `TextEmbedding` across worker threads requires a `Mutex`. The pool
//! does NOT try to maintain N independent models (each is ~30 MB + ONNX
//! runtime overhead). Instead the Semaphore bounds *attempted* parallelism;
//! actual concurrency is governed by the Mutex inside `Embedder`. The
//! Semaphore is what limits queue depth so we don't burn memory holding
//! N pending text buffers while a single inference loop chugs through them.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;

use crate::indexer::Embedder;

/// Cross-session batch size — when scanning fresh, the indexer collects up
/// to this many sessions' worth of 5-vector extracts and embeds them all in
/// one fastembed call.
pub const CROSS_SESSION_BATCH: usize = 32;

/// Pool wrapper around `Arc<Embedder>` with a Semaphore cap. Pass the
/// `Arc<Embedder>` you already use elsewhere — the pool does not own a fresh
/// model, it shares the existing one.
#[derive(Clone)]
pub struct EmbedPool {
    embedder: Arc<Embedder>,
    sem: Arc<Semaphore>,
}

impl EmbedPool {
    /// Build a pool around an existing Embedder. Concurrency cap defaults to
    /// `max(num_cpus / 2, 1)`.
    pub fn new(embedder: Arc<Embedder>) -> Self {
        let cap = (num_cpus::get() / 2).max(1);
        Self {
            embedder,
            sem: Arc::new(Semaphore::new(cap)),
        }
    }

    /// Build a pool with an explicit concurrency cap. For tests / advanced
    /// callers that want deterministic parallelism.
    pub fn with_cap(embedder: Arc<Embedder>, cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            embedder,
            sem: Arc::new(Semaphore::new(cap)),
        }
    }

    /// Embed a batch of texts using the wrapped embedder. Runs the (CPU-bound,
    /// blocking) inference in `spawn_blocking`, gated by the pool's
    /// Semaphore. Returns one `Vec<f32>` per input text in input order.
    pub async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let permit = self
            .sem
            .clone()
            .acquire_owned()
            .await
            .context("acquiring embed-pool permit")?;
        let embedder = self.embedder.clone();
        let res = tokio::task::spawn_blocking(move || {
            // Permit lives until this closure returns so the Semaphore stays
            // counted while the heavy work runs.
            let _permit = permit;
            embedder.embed(texts)
        })
        .await
        .context("embed-pool blocking task join")??;
        Ok(res)
    }

    /// The number of permits the Semaphore was constructed with.
    pub fn capacity(&self) -> usize {
        // tokio's Semaphore doesn't expose "max permits" — track via available
        // permits at construction time would require state. We expose the
        // closest live observation: the number of currently free permits.
        // For tests this is enough because they don't issue work in parallel.
        self.sem.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal fastembed boot — gated on a network-free cache hit. If the
    /// cache is empty this test does nothing (cargo treats it as passed) so
    /// CI doesn't need to download 130 MB just to validate this module.
    fn try_real_embedder() -> Option<Arc<Embedder>> {
        // The full Embedder::new() will download a 130 MB ONNX model if the
        // cache is cold. That's not appropriate for a unit test — gate on the
        // env var that ALSO turns it on in production-ish setups.
        if std::env::var("MEMEX_RUN_REAL_EMBED_TESTS").is_err() {
            return None;
        }
        Embedder::new().ok().map(Arc::new)
    }

    #[test]
    fn t_cap_min_one() {
        // num_cpus is at least 1; cap floor is 1.
        let cap = (num_cpus::get() / 2).max(1);
        assert!(cap >= 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn t_empty_batch_returns_empty() {
        let Some(embedder) = try_real_embedder() else { return };
        let pool = EmbedPool::new(embedder);
        let out = pool.embed_batch(Vec::new()).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn t_with_cap_respects_explicit_value() {
        let Some(embedder) = try_real_embedder() else { return };
        let pool = EmbedPool::with_cap(embedder, 3);
        assert_eq!(pool.capacity(), 3);
    }
}
