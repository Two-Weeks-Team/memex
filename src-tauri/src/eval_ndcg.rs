//! nDCG@10 evaluation harness skeleton (P4, gates AC-4.1.4 + AC-4.3.2).
//!
//! These tests are intentionally `#[ignore]` because the gate decision is
//! deferred to D-8 (see TODO comments) — they need a human-labeled
//! query+relevant-id set that does not yet exist. The harness shape lives
//! here so the implementation is reviewable now and the labeled data can be
//! dropped in later without changing the eval API.
//!
//! Eval cycle:
//! 1. Load `Vec<LabeledQuery>` from a fixture JSON (TBD path).
//! 2. For each query, run the search variant (baseline dense vs.
//!    `content_late`-augmented vs. ACORN-filtered).
//! 3. Compute nDCG@10 per query and average across queries.
//! 4. Compare variants and report the delta. The numeric thresholds in the
//!    spec (+15% nDCG for KB-01, +20% recall for KB-04) are checked here.

use serde::{Deserialize, Serialize};

/// A single labeled query for retrieval evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabeledQuery {
    /// Free-text query the user would type.
    pub query: String,
    /// Session IDs that should appear in the top results, in descending
    /// relevance order (most relevant first). Empty list = no signal.
    pub relevant_ids: Vec<String>,
}

/// Compute nDCG@10 given:
/// - `actual`: ranked list of session IDs returned by the search
/// - `labels`: ranked list of relevant session IDs from the labeled set
///
/// Uses binary relevance derived from set membership in `labels` for
/// simplicity. A graded version could weight by `labels` position; we keep
/// it binary so the first integration of the harness has minimal moving parts.
pub fn ndcg_at_10(actual: &[String], labels: &[String]) -> f64 {
    const K: usize = 10;
    let relevant: std::collections::HashSet<&str> = labels.iter().map(|s| s.as_str()).collect();
    if relevant.is_empty() {
        return 0.0;
    }
    // DCG over the actual ranking
    let mut dcg = 0.0_f64;
    for (i, sid) in actual.iter().take(K).enumerate() {
        if relevant.contains(sid.as_str()) {
            // Binary gain: 1.0 if relevant. Rank position i is 0-indexed
            // → use log2(i+2) per the standard DCG formula.
            dcg += 1.0 / ((i + 2) as f64).log2();
        }
    }
    // IDCG — ideal DCG if all relevant items were ranked first.
    let n = relevant.len().min(K);
    let mut idcg = 0.0_f64;
    for i in 0..n {
        idcg += 1.0 / ((i + 2) as f64).log2();
    }
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

/// Aggregate nDCG@10 across a labeled query set. Returns the mean. NaN-safe
/// (zeroes are treated as zero contribution).
pub fn mean_ndcg_at_10<F>(labeled: &[LabeledQuery], mut run_query: F) -> f64
where
    F: FnMut(&str) -> Vec<String>,
{
    if labeled.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0_f64;
    for q in labeled {
        let actual = run_query(&q.query);
        sum += ndcg_at_10(&actual, &q.relevant_ids);
    }
    sum / (labeled.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ndcg_at_10 unit tests (NOT ignored — these test the metric itself).

    #[test]
    fn ndcg_perfect_ranking_is_one() {
        let actual: Vec<String> = vec!["a", "b", "c"].into_iter().map(String::from).collect();
        let labels = actual.clone();
        let s = ndcg_at_10(&actual, &labels);
        assert!((s - 1.0).abs() < 1e-9, "expected 1.0, got {s}");
    }

    #[test]
    fn ndcg_no_relevant_in_topk_is_zero() {
        let actual: Vec<String> =
            vec!["x", "y", "z"].into_iter().map(String::from).collect();
        let labels: Vec<String> = vec!["a", "b"].into_iter().map(String::from).collect();
        let s = ndcg_at_10(&actual, &labels);
        assert!(s.abs() < 1e-9);
    }

    #[test]
    fn ndcg_handles_empty_labels() {
        let actual: Vec<String> = vec!["a".into()];
        let labels: Vec<String> = Vec::new();
        assert_eq!(ndcg_at_10(&actual, &labels), 0.0);
    }

    #[test]
    fn ndcg_partial_match_in_range() {
        // 1 relevant of 2 is in top-K. Score should be in (0, 1).
        let actual: Vec<String> =
            vec!["a", "x", "y"].into_iter().map(String::from).collect();
        let labels: Vec<String> = vec!["a", "b"].into_iter().map(String::from).collect();
        let s = ndcg_at_10(&actual, &labels);
        assert!(s > 0.0 && s < 1.0, "got {s}");
    }

    // ---- Eval gate tests — `#[ignore]` until D-8 labeled data lands.

    /// TODO P4-EVAL D-8: requires labeled dataset. Once `eval/labels.json`
    /// (or similar) exists, load it and run dense-only baseline.
    #[test]
    #[ignore = "P4-EVAL D-8: requires labeled dataset"]
    fn eval_ndcg_dense_only_baseline() {
        // Pseudocode:
        //   let labels = load_labels("eval/labels.json")?;
        //   let baseline = mean_ndcg_at_10(&labels, |q| run_dense_only(q));
        //   println!("dense-only nDCG@10 = {baseline}");
        unreachable!("requires labeled dataset");
    }

    /// TODO P4-EVAL D-8: requires labeled dataset. Verify KB-01 +15% nDCG@10.
    #[test]
    #[ignore = "P4-EVAL D-8: requires labeled dataset"]
    fn eval_ndcg_with_content_late() {
        // Pseudocode:
        //   let labels = load_labels("eval/labels.json")?;
        //   let baseline = mean_ndcg_at_10(&labels, |q| run_dense_only(q));
        //   let augmented = mean_ndcg_at_10(&labels, |q| run_with_content_late(q));
        //   let delta = (augmented - baseline) / baseline;
        //   assert!(delta >= 0.15, "AC-4.1.4: nDCG@10 must improve by ≥15%");
        unreachable!("requires labeled dataset");
    }

    /// TODO P4-EVAL D-8: requires labeled dataset. Verify KB-04 +20% recall@10.
    #[test]
    #[ignore = "P4-EVAL D-8: requires labeled dataset"]
    fn eval_recall_filtered_acorn() {
        // Pseudocode:
        //   let labels = load_labels("eval/labels.json")?;
        //   let baseline = recall_at_10_unfiltered(&labels);
        //   let acorn = recall_at_10_with_acorn(&labels);
        //   let delta = (acorn - baseline) / baseline;
        //   assert!(delta >= 0.20, "AC-4.3.2: filtered recall must improve by ≥20%");
        unreachable!("requires labeled dataset");
    }
}
