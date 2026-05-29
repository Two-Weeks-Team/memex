//! Issue #13 — quantization-mode sweep benchmark.
//!
//! Compares recall / p95 latency / index size across `MEMEX_QUANT_MODE`
//! values (`f32` / `tq-bits1` / `tq-bits2`) against a labeled-queries
//! fixture. Replaces the broken `cargo run -- eval ndcg` recipe in
//! `docs/benchmarks.md` (PR #12 REV-13 had to retract that command
//! because `eval_ndcg` is a library module, not a binary).
//!
//! ## Modes
//!
//! - **Dry-run** (default): prints the resolved `QuantMode`, registers a
//!   no-op Criterion bench so the harness exits cleanly. No Qdrant or
//!   corpus required. Used by CI to verify the bench compiles and links
//!   against the current `memex_lib`.
//!
//! - **Live** (`MEMEX_BENCH_LIVE=1`): loads `fixtures/labeled-queries.jsonl`
//!   and runs the timed bench. Requires a running Qdrant on `:6334` and an
//!   indexed corpus matching the labeled-queries session IDs.
//!
//! ## Quick start
//!
//! ```text
//! # Compile check + dry-run (CI-safe; no Qdrant needed):
//! cargo bench --bench quant_sweep
//!
//! # Live sweep across the three modes (needs Qdrant Docker + corpus):
//! for mode in f32 tq-bits1 tq-bits2; do
//!     MEMEX_BENCH_LIVE=1 MEMEX_QUANT_MODE=$mode \
//!         cargo bench --bench quant_sweep
//! done
//! ```
//!
//! Reports land in `target/criterion/quant/<mode>/report/index.html`.
//!
//! ## Live-mode pipeline (Issue #13 — second pass, fully wired)
//!
//! When `MEMEX_BENCH_LIVE=1`, the bench:
//!
//! 1. Connects to Qdrant (`MEMEX_QDRANT_URL` or `http://127.0.0.1:6334`)
//! 2. **Drops** the `memex_sessions_v3` collection so the next
//!    `ensure_collection_v3()` recreates it with the *current*
//!    `MEMEX_QUANT_MODE` (the quant config is read inside the create call,
//!    so an existing collection's config would stick otherwise).
//! 3. Initialises the BGE-small embedder (first run downloads ~130 MB into
//!    `~/.cache/fastembed/`).
//! 4. Scans `examples/sample-corpus/` and bulk-indexes its 12 sessions.
//! 5. Registers a Criterion bench that, on each iteration, picks the next
//!    labeled query (round-robin) and calls `indexer::lens_search` against
//!    the freshly-indexed collection. Criterion derives p50 / p95 / outlier
//!    statistics from the sample.
//! 6. After the bench, runs every labeled query once more (not timed) and
//!    computes `eval_ndcg::ndcg_at_10` against `relevant_ids`. The mean is
//!    printed alongside the criterion report path for the human reader.
//!
//! All quant-mode-specific work — collection drop + recreate — happens
//! *before* `bench_function` so the timed loop only measures the query, not
//! cold-start IO.

use std::path::PathBuf;
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use memex_lib::indexer::{self, Embedder, LensWeights};
use memex_lib::schema::QuantMode;
use memex_lib::{crud, eval_ndcg, parser};

fn quant_sweep(c: &mut Criterion) {
    let mode = QuantMode::from_env();
    let live = std::env::var("MEMEX_BENCH_LIVE")
        .ok()
        .as_deref()
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE"))
        .unwrap_or(false);

    println!("[quant_sweep] MEMEX_QUANT_MODE resolved to: {}", mode.as_name());
    println!("[quant_sweep] MEMEX_BENCH_LIVE = {live}");

    if !live {
        println!(
            "[quant_sweep] DRY-RUN. Set MEMEX_BENCH_LIVE=1 to run the actual \
             sweep (requires Qdrant on :6334 + an indexed corpus). The \
             harness still registers a no-op bench so `cargo bench` exits 0 \
             and the criterion report carries the mode dimension."
        );
        let mut group = c.benchmark_group(format!("quant/{}", mode.as_name()));
        group.bench_function("dry_run_noop", |b| {
            b.iter(|| std::hint::black_box(1u64))
        });
        group.finish();
        return;
    }

    // ── Live mode ─────────────────────────────────────────────────────────
    let queries = load_labeled_queries();
    println!(
        "[quant_sweep] loaded {} labeled queries from fixtures/labeled-queries.jsonl",
        queries.len()
    );
    assert!(!queries.is_empty(), "live mode requires a non-empty fixture");

    // Single tokio runtime for all async calls — created once, reused inside
    // `b.iter` via `block_on`. `Builder::new_current_thread` is enough for
    // a single-flight bench harness; we don't need work-stealing here.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    // 1. Connect + 2. drop existing collection so the recreate picks up the
    //    current QuantMode + 3. embedder init + 4. corpus indexing.
    //
    // Corpus path resolution:
    //   - `MEMEX_CORPUS_DIR` env var (absolute or `~`-prefixed) → use that
    //   - otherwise fall back to the in-repo synthetic sample-corpus
    //
    // The labeled-queries fixture is hard-coupled to sample-corpus session IDs,
    // so nDCG is only meaningful when the env var is unset (or explicitly
    // points back at sample-corpus). For production-scale runs the bench still
    // reports indexing throughput + p95 query latency from Criterion; nDCG is
    // suppressed with a "N/A" marker so the reader isn't misled.
    let sample_corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("examples/sample-corpus");
    let corpus_path = match std::env::var("MEMEX_CORPUS_DIR").ok().filter(|s| !s.is_empty()) {
        Some(raw) => {
            let expanded = if let Some(rest) = raw.strip_prefix("~/") {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(rest))
                    .unwrap_or_else(|_| PathBuf::from(&raw))
            } else if raw == "~" {
                std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(&raw))
            } else {
                PathBuf::from(&raw)
            };
            eprintln!(
                "[quant_sweep] MEMEX_CORPUS_DIR set → using corpus at {} (nDCG suppressed unless this matches the sample-corpus labeled-queries fixture)",
                expanded.display()
            );
            expanded
        }
        None => sample_corpus.clone(),
    };
    let corpus_is_sample = corpus_path == sample_corpus;
    let (qdrant, embedder) = rt.block_on(async {
        eprintln!("[quant_sweep] connecting to Qdrant…");
        let q = Arc::new(indexer::connect().await.expect("Qdrant connect"));

        eprintln!(
            "[quant_sweep] dropping collection memex_sessions_v3 so recreate \
             picks up MEMEX_QUANT_MODE={}",
            mode.as_name()
        );
        // Best-effort delete; ignore "not found" on first run. Use the
        // shared `COLLECTION_V3` constant so this can't drift from the
        // name `crud::ensure_collection_v3` recreates next (Gemini #23
        // review).
        let _ = q.delete_collection(memex_lib::schema::COLLECTION_V3).await;

        crud::ensure_collection_v3(&q)
            .await
            .expect("ensure_collection_v3");

        eprintln!("[quant_sweep] loading BGE-small embedder (first run: ~130MB download)…");
        let e = Arc::new(Embedder::new().expect("Embedder::new"));

        eprintln!(
            "[quant_sweep] scanning {} for sessions…",
            corpus_path.display()
        );
        let sessions = parser::scan_dir(&corpus_path).expect("scan corpus");
        eprintln!(
            "[quant_sweep] indexing {} session(s) into v3 collection…",
            sessions.len()
        );
        let report = indexer::bulk_index_arc(&q, e.clone(), &sessions)
            .await
            .expect("bulk_index_arc");
        eprintln!(
            "[quant_sweep] indexed {} session(s) ({} skipped)",
            report.indexed,
            sessions.len().saturating_sub(report.indexed)
        );
        (q, e)
    });

    // 5. Timed bench — round-robin over the labeled queries.
    let weights = LensWeights::default();
    let mut q_idx = 0usize;
    let mut group = c.benchmark_group(format!("quant/{}", mode.as_name()));
    group.bench_function("query_p95", |b| {
        b.iter(|| {
            // Bound q_idx to queries.len() at increment time so the index
            // can't grow without limit and trigger an overflow panic on
            // very long sample runs in debug profile (Gemini #23 review).
            let q_text = &queries[q_idx].query;
            q_idx = (q_idx + 1) % queries.len();
            let hits = rt
                .block_on(async {
                    indexer::lens_search(&qdrant, &embedder, q_text, &weights, 10, 50).await
                })
                .expect("lens_search");
            std::hint::black_box(hits.len())
        })
    });
    group.finish();

    // 6. nDCG measurement — separate pass over every query, not timed.
    //    Only meaningful when the corpus is sample-corpus (labeled queries
    //    reference its session IDs). For production-scale corpora we still
    //    run the queries to exercise the search path, but report N/A.
    if corpus_is_sample {
        let mut ndcg_sum = 0.0_f64;
        for q in &queries {
            let actual: Vec<String> = rt
                .block_on(async {
                    indexer::lens_search(&qdrant, &embedder, &q.query, &weights, 10, 50).await
                })
                .expect("lens_search (ndcg pass)")
                .into_iter()
                .map(|h| h.session_id)
                .collect();
            ndcg_sum += eval_ndcg::ndcg_at_10(&actual, &q.relevant_ids);
        }
        let mean_ndcg = ndcg_sum / (queries.len() as f64);
        println!(
            "[quant_sweep] RESULT  mode={}  nDCG@10={:.4}  corpus={}  queries={}",
            mode.as_name(),
            mean_ndcg,
            corpus_path.display(),
            queries.len()
        );
    } else {
        // Production-scale path: exercise lens_search once per query (untimed)
        // so any panics surface, then report N/A. Criterion's `query_p95` group
        // above already captured the timed measurement.
        for q in &queries {
            let _ = rt
                .block_on(async {
                    indexer::lens_search(&qdrant, &embedder, &q.query, &weights, 10, 50).await
                })
                .expect("lens_search (production-scale shake-out)");
        }
        println!(
            "[quant_sweep] RESULT  mode={}  nDCG@10=N/A (corpus != sample-corpus; labeled-queries fixture not matched)  corpus={}  queries={}",
            mode.as_name(),
            corpus_path.display(),
            queries.len()
        );
    }
    println!(
        "[quant_sweep] criterion report: target/criterion/quant/{}/query_p95/report/index.html",
        mode.as_name()
    );
}

// Gemini #23 review: there's already a public `LabeledQuery` in
// `eval_ndcg` with `pub query` + `pub relevant_ids` + `Deserialize`.
// Re-aliasing avoids duplicating the struct (and the risk of drift if
// the canonical one gains a field). The rest of the bench still uses
// the local name `LabeledQuery` unchanged.
type LabeledQuery = eval_ndcg::LabeledQuery;

fn load_labeled_queries() -> Vec<LabeledQuery> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("labeled-queries.jsonl");
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) => {
            println!(
                "[quant_sweep] could not read {}: {}. Returning empty set.",
                path.display(),
                e,
            );
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        match serde_json::from_str::<LabeledQuery>(trimmed) {
            Ok(q) => out.push(q),
            Err(e) => println!(
                "[quant_sweep] skipping line {} of labeled-queries.jsonl: {}",
                i + 1,
                e,
            ),
        }
    }
    out
}

criterion_group!(benches, quant_sweep);
criterion_main!(benches);
