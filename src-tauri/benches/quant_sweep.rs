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
//! ## Live-mode wiring status
//!
//! The query-runner + nDCG computation closures are documented stubs in
//! this commit. Wiring them to the actual `memex_lib::indexer::lens_search`
//! + `memex_lib::eval_ndcg::mean_ndcg_at_10` path is deliberately deferred
//! to a follow-up: it depends on collection setup state + embedder
//! initialisation that varies by deployment. Shipping the runtime config
//! (`MEMEX_QUANT_MODE` env in `schema::QuantMode`) + the bench scaffold +
//! the fixture schema independently lets ops teams measure on their own
//! corpora without waiting for our reference numbers.

use criterion::{criterion_group, criterion_main, Criterion};
use memex_lib::schema::QuantMode;

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

    // Live mode — load the fixture + register the timed bench.
    let queries = load_labeled_queries();
    println!(
        "[quant_sweep] loaded {} labeled queries from fixtures/labeled-queries.jsonl",
        queries.len()
    );
    if queries.is_empty() {
        println!(
            "[quant_sweep] fixture is empty — add entries to \
             src-tauri/fixtures/labeled-queries.jsonl before running live."
        );
    }

    let mut group = c.benchmark_group(format!("quant/{}", mode.as_name()));
    group.bench_function("query_p95", |b| {
        b.iter(|| {
            // STUB — see module-level "Live-mode wiring status" note. Real
            // impl would:
            //   1. Ensure v3 collection exists with the chosen QuantMode
            //   2. Index the corpus subset referenced by the labeled fixture
            //   3. For each query, call `indexer::lens_search` and collect
            //      wall-clock timing
            //   4. Return one sample per `b.iter` invocation; criterion
            //      derives p50/p95 statistics
            std::hint::black_box(queries.len())
        })
    });
    group.finish();

    println!(
        "[quant_sweep] nDCG@10 measurement stub — wire to \
         memex_lib::eval_ndcg::mean_ndcg_at_10 with a closure that runs \
         each query against the live Qdrant + returns the actual session \
         IDs (see eval_ndcg::LabeledQuery for the relevance shape)."
    );
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)] // fields used once live-mode wiring lands
struct LabeledQuery {
    query: String,
    relevant_ids: Vec<String>,
}

fn load_labeled_queries() -> Vec<LabeledQuery> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
