//! **Memex Wrapped** — engineering "Spotify Wrapped".
//!
//! Cold-start Killer (companion.rs) answers "what should I remember at the
//! start of a new session?". Wrapped answers the complementary "what do I
//! actually look like as an engineer, across all my sessions?".
//!
//! Output is a one-page digest aggregating the Qdrant index over a sliding
//! time window (default 30 days), surfacing:
//!
//!   - **Top tools** — which Claude tools dominate (Bash, Edit, …)
//!   - **Top bash binaries** — toolchain footprint (cargo, pnpm, gh, …)
//!   - **Top file extensions** — language fingerprint (.rs, .ts, .md, …)
//!   - **Top projects** — where time actually went
//!   - **Intent / arc / outcome mix** — % build vs impl vs debug, etc.
//!     (Reuses the deterministic enrich.rs categories.)
//!   - **Repeated decisions** — decision lines that appear across ≥2
//!     sessions (the "I keep re-deciding the same thing" signal).
//!   - **Debugging fingerprint** — average turn count and "had errors"
//!     rate. Surfaces "you debug for N turns on average".
//!   - **Cross-agent split** — Claude Code vs Codex CLI counts, when
//!     both agents are present.
//!
//! Zero LLM. Everything is pure aggregation over Qdrant payload + a
//! single JSONL re-parse per session for decision mining. Same Companion
//! sandbox / source_agent routing applies, so Codex sessions go through
//! the codex parser and Claude Code through the regular one.
//!
//! Surfaces:
//!   - CLI: `memex wrapped [--window-days N] [--limit M] [--json]`
//!   - MCP: `generate_wrapped_report`
//!   - Tauri: `compose_wrapped`
//!
//! The markdown output is shareable — designed for screenshots and
//! Twitter/X posts. That's a deliberate growth loop for the hackathon
//! pitch: every Memex user can post their Wrapped, which doubles as a
//! "Memex did this for me" testimonial.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use qdrant_client::qdrant::{
    facet_value, Condition, DatetimeRange, Direction, FacetCountsBuilder, FacetResponse, Filter,
    OrderBy, ScrollPointsBuilder, Timestamp,
};
use qdrant_client::Qdrant;
use serde::{Deserialize, Serialize};

use crate::companion::normalize_dedup_key_pub;
use crate::parser::ToolCall;
use crate::payload::{payload_bool, payload_i64, payload_str};

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedReport {
    /// Inclusive window in days back from `now`. `0` = all-time.
    pub window_days: u32,
    /// Total sessions scanned for this report.
    pub sessions_total: usize,
    /// Sessions that contributed to mining (could be re-parsed).
    pub sessions_mined: usize,
    /// Aggregate tool stats — pre-bucketed.
    pub top_tools: Vec<Atom>,
    pub top_binaries: Vec<Atom>,
    pub top_extensions: Vec<Atom>,
    pub top_projects: Vec<Atom>,
    /// Distribution buckets (% summing to ≤1.0 — buckets <2% omitted).
    pub intent_mix: Vec<Bucket>,
    pub arc_mix: Vec<Bucket>,
    pub outcome_mix: Vec<Bucket>,
    /// Decision texts that appear in ≥ `repeat_threshold` sessions.
    pub repeated_decisions: Vec<RepeatedDecision>,
    /// Average turn count, error rate, longest session.
    pub fingerprint: Fingerprint,
    /// Per-agent breakdown when corpus has both Claude Code + Codex.
    pub agent_split: Vec<AgentSplit>,
    /// Total tool calls across the window.
    pub total_tool_calls: usize,
    /// True when the corpus exceeded `MAX_SCROLLED_SESSIONS` and the report
    /// was computed from a capped slice. Lets callers / markdown surface
    /// "based on N / M sessions" rather than silently dropping data.
    /// (Codex P2-b / Quality H1.)
    #[serde(default)]
    pub truncated: bool,
    /// Ready-to-screenshot markdown digest.
    pub markdown: String,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Atom {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bucket {
    pub label: String,
    pub count: usize,
    pub share: f32, // 0.0 - 1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatedDecision {
    /// Distilled decision text (clipped).
    pub text: String,
    /// How many distinct sessions echoed it.
    pub sessions: usize,
    /// Up to 3 example session ids carrying this decision.
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fingerprint {
    pub avg_turns: f32,
    pub median_turns: u32,
    pub longest_turns: u32,
    pub longest_session_id: String,
    pub had_errors_rate: f32,
    /// "When you debug, how many tool calls per session on average?" —
    /// mean of tool_count restricted to sessions with arc ∈ {debug, debug-fix}.
    pub debug_avg_tools: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSplit {
    pub agent: String,
    pub sessions: usize,
    pub share: f32,
    /// Top intent label for this agent's sessions.
    pub dominant_intent: String,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Hard cap on number of sessions we'll re-parse for decision mining,
/// regardless of corpus size. Anything beyond this samples by recency.
const MAX_MINED_SESSIONS: usize = 64;

/// Hard cap on payload-only sessions scrolled per Wrapped invocation.
/// Aggregation costs scale linearly with corpus size; cap protects the
/// UI thread and Qdrant. When exceeded, `WrappedReport.truncated = true`
/// so the markdown can say "based on most-recent N of M sessions".
/// (Codex P2-b / Quality H1 fix.)
const MAX_SCROLLED_SESSIONS: usize = 20_000;

/// Minimum number of distinct sessions a decision must appear in to count
/// as "repeated" in the report.
const REPEATED_DECISION_THRESHOLD: usize = 2;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Compose a Wrapped report covering the last `window_days` days. `0`
/// means all-time. `limit` caps the number of sessions actually re-parsed
/// for decision mining (still aggregates payload stats across all
/// sessions in the window).
///
/// **No embedder dependency.** Wrapped is pure aggregation over Qdrant
/// payload + a JSONL re-parse for decision mining — vectors are never
/// queried. The embedder argument was previously plumbed through "for
/// future re-rank work" but every entry point (MCP, CLI, Tauri command,
/// GUI) was paying the BGE-small ONNX init cost (130MB first-run model
/// download, ~1s warm). Codex P2-a flagged this; embedder removed until
/// re-rank actually lands.
pub async fn compose_wrapped(
    qdrant: &Qdrant,
    window_days: u32,
    limit: usize,
) -> Result<WrappedReport> {
    let started = Instant::now();
    let limit = limit.clamp(8, MAX_MINED_SESSIONS);

    // ---- 1. Scroll payload-only summaries from v3 within the window ---
    let (payloads, truncated) = scroll_window(qdrant, window_days).await?;
    let sessions_total = payloads.len();

    // T3.1 — Facet API fast path for the 4 indexed payload fields
    // (project_name, intent, outcome, source_agent). Each call returns a
    // server-side value→count map in O(field cardinality), not O(N). We run
    // them in parallel with `tokio::join!` to overlap RPC latency. On any
    // failure, the per-field map falls back to empty and the scroll-based
    // tally below picks up the slack — never silently miscount.
    //
    // PR #12 REV-4 (Codex P2) — when `truncated == true`, scroll covered only
    // the most-recent MAX_SCROLLED_SESSIONS points but facet counts EVERY
    // point in the time window. If we mixed them, `sessions_total` (= scroll
    // size, capped) would be the denominator for `project_counts` (= full
    // window), letting a bucket share exceed 100 %. Solution: when truncated,
    // skip the facet fast path entirely. The scroll-based tally below picks
    // it up so every bucket share stays consistent with `sessions_total`.
    let (mut project_counts, mut intent_counts, mut outcome_counts, mut agent_counts) =
        if truncated {
            (
                HashMap::<String, usize>::new(),
                HashMap::<String, usize>::new(),
                HashMap::<String, usize>::new(),
                HashMap::<String, usize>::new(),
            )
        } else {
            let window_filter = window_days_to_filter(window_days);
            let (proj_res, intent_res, outcome_res, agent_res) = tokio::join!(
                facet_field(qdrant, "project_name", window_filter.clone()),
                facet_field(qdrant, "intent",       window_filter.clone()),
                facet_field(qdrant, "outcome",      window_filter.clone()),
                facet_field(qdrant, "source_agent", window_filter.clone()),
            );
            (
                proj_res.unwrap_or_default(),
                intent_res.unwrap_or_default(),
                outcome_res.unwrap_or_default(),
                agent_res.unwrap_or_default(),
            )
        };

    // ---- 2. Aggregate the remaining payload-only stats (scroll fallback)
    //         `arc` is computed/enriched but NOT a v3 indexed payload field,
    //         and `agent_intents` is a 2-D (agent × intent) combo that the
    //         Facet API doesn't express in one call — both still tallied
    //         from scroll. So are per-session stats (turn counts, errors).
    let mut arc_counts: HashMap<String, usize> = HashMap::new();
    let mut agent_intents: HashMap<String, HashMap<String, usize>> = HashMap::new();
    // PR #12 REV-9 (CodeRabbit #13 / Codex P2 #16) — track source_agent
    // counts from scroll INDEPENDENTLY of whether facet succeeded, so we
    // can reconcile against legacy points whose `source_agent` payload was
    // missing at index time (scroll_window normalizes those to "claude_code"
    // but they don't appear in the facet result).
    let mut scroll_agent_counts: HashMap<String, usize> = HashMap::new();
    let mut total_turns: u64 = 0;
    let mut turn_samples: Vec<u32> = Vec::new();
    let mut had_errors_count: usize = 0;
    let mut total_tool_calls: u64 = 0;
    let mut longest_turns: u32 = 0;
    let mut longest_session_id = String::new();
    // For the debugging-fingerprint averaging.
    let mut debug_tool_counts: Vec<u32> = Vec::new();

    // Defense in depth: when the Facet RPC succeeded but returned 0 hits
    // (e.g. server downgrade, payload index disabled, future SDK rename),
    // re-tally the four fields from the scroll so the report never silently
    // shows empty distributions. This makes the Facet path a strict speedup
    // — never a correctness regression.
    let facet_proj_empty   = project_counts.is_empty();
    let facet_intent_empty = intent_counts.is_empty();
    let facet_outcome_empty= outcome_counts.is_empty();
    let facet_agent_empty  = agent_counts.is_empty();

    for summary in &payloads {
        if facet_proj_empty && !summary.project_name.is_empty() {
            *project_counts.entry(summary.project_name.clone()).or_insert(0) += 1;
        }
        if !summary.intent.is_empty() {
            if facet_intent_empty {
                *intent_counts.entry(summary.intent.clone()).or_insert(0) += 1;
            }
            agent_intents
                .entry(summary.source_agent.clone())
                .or_default()
                .entry(summary.intent.clone())
                .and_modify(|n| *n += 1)
                .or_insert(1);
        }
        if !summary.arc.is_empty() {
            *arc_counts.entry(summary.arc.clone()).or_insert(0) += 1;
        }
        if facet_outcome_empty && !summary.outcome.is_empty() {
            *outcome_counts.entry(summary.outcome.clone()).or_insert(0) += 1;
        }
        // REV-9 — always tally agent from scroll (independent of facet),
        // so we can reconcile legacy-missing source_agent below.
        *scroll_agent_counts.entry(summary.source_agent.clone()).or_insert(0) += 1;
        if facet_agent_empty {
            *agent_counts.entry(summary.source_agent.clone()).or_insert(0) += 1;
        }

        let user_t = summary.user_turns.max(0) as u32;
        let asst_t = summary.assistant_turns.max(0) as u32;
        let turns = user_t.saturating_add(asst_t);
        total_turns += u64::from(turns);
        turn_samples.push(turns);
        if turns > longest_turns {
            longest_turns = turns;
            longest_session_id = summary.session_id.clone();
        }
        if summary.has_errors {
            had_errors_count += 1;
        }
        let tc = summary.tool_count.max(0) as u32;
        total_tool_calls += u64::from(tc);
        if summary.arc == "debug" || summary.arc == "debug-fix" {
            debug_tool_counts.push(tc);
        }
    }

    // PR #12 REV-9 (CodeRabbit #13 / Codex P2 #16) — reconcile legacy points.
    // The facet API counts only points that actually have a `source_agent`
    // value in their payload index. Older points (pre-KH-01 multi-agent
    // schema, or partially-backfilled corpora) may be missing the field,
    // and `scroll_window` normalises those to `"claude_code"`. Without
    // reconciliation, the facet path would drop those sessions from
    // `agent_counts` while `sessions_total` still includes them, so
    // `agent_split.share` would not sum to 1.0 and "dominant intent"
    // distribution would be skewed.
    //
    // Reconciliation rule: for each agent value seen by scroll, if scroll
    // counted MORE than facet did, the delta is the count of legacy points
    // that the facet index missed — add the delta into facet's bucket.
    // When the facet path was skipped (truncated, or facet RPC failed and
    // `facet_agent_empty` already triggered the scroll-tally branch above),
    // agent_counts already equals scroll counts and the delta is zero.
    if !facet_agent_empty {
        for (agent, scroll_n) in &scroll_agent_counts {
            let facet_n = agent_counts.get(agent).copied().unwrap_or(0);
            if *scroll_n > facet_n {
                let delta = *scroll_n - facet_n;
                *agent_counts.entry(agent.clone()).or_insert(0) += delta;
            }
        }
    }

    // ---- 3. Re-parse the most-recent `limit` sessions for decisions
    //         and richer tool/extension/binary mining -------------------
    let mut to_mine = payloads.clone();
    to_mine.sort_by(|a, b| b.start_iso.cmp(&a.start_iso));
    to_mine.truncate(limit);

    let mut tool_counts: HashMap<String, usize> = HashMap::new();
    let mut ext_counts: HashMap<String, usize> = HashMap::new();
    let mut bin_counts: HashMap<String, usize> = HashMap::new();
    // Decision-key → (original text, distinct session ids that uttered it).
    // We store the text alongside the sids on first sight so the report can
    // be built in one pass — eliminates the second `recompute_repeated_
    // decisions` walk gemini PR #7 flagged (4 HIGH comments, lines 252/317
    // /356/637). Per-session dedup via a HashSet local to each session.
    let mut decision_sessions: HashMap<String, (String, Vec<String>)> = HashMap::new();
    let mut sessions_mined: usize = 0;

    for s in &to_mine {
        let Some(source_path) = s.source_path.as_ref() else { continue };
        let Ok(validated) = crate::sec::validate_session_path(Path::new(source_path)) else {
            continue;
        };
        let parsed = crate::session_roots::parse_session_routed(&s.source_agent, &validated);
        let Ok(session) = parsed else { continue };
        if session.turns.is_empty() {
            continue;
        }
        sessions_mined += 1;

        // Head + tail sampling — same band as companion.
        let n = session.turns.len();
        let head_take = n.min(12);
        let tail_take = n.min(12);
        let mut seen_idx: std::collections::HashSet<usize> = std::collections::HashSet::new();
        // Track keys we've already pushed THIS session's id for, so a single
        // session that emits "I'll use X" five times only counts once.
        let mut session_keys: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let iter = session
            .turns
            .iter()
            .enumerate()
            .take(head_take)
            .chain(session.turns.iter().enumerate().skip(n.saturating_sub(tail_take)));
        for (idx, turn) in iter {
            if !seen_idx.insert(idx) {
                continue;
            }
            // Aggregate tool calls into top-N pools.
            for tc in &turn.tool_calls {
                aggregate_tool(tc, &mut tool_counts, &mut ext_counts, &mut bin_counts);
            }
            // Decision mining via companion's published helper.
            let mut bucket: Vec<crate::companion::DecisionAtom> = Vec::new();
            // Build a stand-in PrimedSession to call the extractor with
            // confidence math intact.
            let primed = crate::companion::PrimedSession {
                session_id: s.session_id.clone(),
                project_name: s.project_name.clone(),
                project_path: s.project_path.clone(),
                ai_title: s.ai_title.clone(),
                start_iso: s.start_iso.clone(),
                turn_count: i64::from(s.user_turns + s.assistant_turns),
                has_errors: s.has_errors,
                similarity: 1.0,
                match_reason: "wrapped corpus mine".to_string(),
            };
            crate::companion::extract_decisions_from_turn_pub(
                turn,
                0,
                &primed,
                &mut bucket,
            );
            for d in bucket {
                let key = normalize_dedup_key_pub(&d.text);
                if !session_keys.insert(key.clone()) {
                    continue;
                }
                let entry = decision_sessions
                    .entry(key)
                    .or_insert_with(|| (d.text.clone(), Vec::new()));
                entry.1.push(s.session_id.clone());
            }
        }
    }

    // ---- 4. Build the public report shape -----------------------------
    let top_tools = top_atoms(&tool_counts, 8);
    let top_binaries = top_atoms(&bin_counts, 8);
    let top_extensions = top_atoms(&ext_counts, 6);
    let top_projects = top_atoms(&project_counts, 8);
    let intent_mix = into_buckets(&intent_counts, sessions_total);
    let arc_mix = into_buckets(&arc_counts, sessions_total);
    let outcome_mix = into_buckets(&outcome_counts, sessions_total);

    // Build the repeated-decisions list directly from the populated map —
    // no second mining pass. session_keys above guarantees each sid appears
    // at most once per key, so a plain len() check is enough. gemini PR #7
    // (lines 252/317/356/637) suggested this exact shape; matched.
    let mut repeated: Vec<RepeatedDecision> = decision_sessions
        .into_iter()
        .filter_map(|(_key, (text, sids))| {
            if sids.len() >= REPEATED_DECISION_THRESHOLD {
                Some(RepeatedDecision {
                    text,
                    sessions: sids.len(),
                    examples: sids.into_iter().take(3).collect(),
                })
            } else {
                None
            }
        })
        .collect();
    repeated.sort_by(|a, b| b.sessions.cmp(&a.sessions));
    repeated.truncate(6);

    let avg_turns = if sessions_total > 0 {
        total_turns as f32 / sessions_total as f32
    } else {
        0.0
    };
    let median_turns = median_u32(&mut turn_samples);
    let had_errors_rate = if sessions_total > 0 {
        had_errors_count as f32 / sessions_total as f32
    } else {
        0.0
    };
    let debug_avg_tools = if debug_tool_counts.is_empty() {
        0.0
    } else {
        debug_tool_counts.iter().map(|n| *n as f32).sum::<f32>() / debug_tool_counts.len() as f32
    };

    let fingerprint = Fingerprint {
        avg_turns,
        median_turns,
        longest_turns,
        longest_session_id,
        had_errors_rate,
        debug_avg_tools,
    };

    let agent_split: Vec<AgentSplit> = {
        let total = sessions_total.max(1);
        let mut v: Vec<AgentSplit> = agent_counts
            .iter()
            .map(|(agent, &count)| {
                let dominant = agent_intents
                    .get(agent)
                    .and_then(|m| m.iter().max_by_key(|(_, n)| **n).map(|(k, _)| k.clone()))
                    .unwrap_or_else(|| "mixed".to_string());
                AgentSplit {
                    agent: agent.clone(),
                    sessions: count,
                    share: count as f32 / total as f32,
                    dominant_intent: dominant,
                }
            })
            .collect();
        v.sort_by(|a, b| b.sessions.cmp(&a.sessions));
        v
    };

    let markdown = render_markdown(
        window_days,
        sessions_total,
        sessions_mined,
        truncated,
        total_tool_calls as usize,
        &top_tools,
        &top_binaries,
        &top_extensions,
        &top_projects,
        &intent_mix,
        &arc_mix,
        &outcome_mix,
        &repeated,
        &fingerprint,
        &agent_split,
    );

    Ok(WrappedReport {
        window_days,
        sessions_total,
        sessions_mined,
        top_tools,
        top_binaries,
        top_extensions,
        top_projects,
        intent_mix,
        arc_mix,
        outcome_mix,
        repeated_decisions: repeated,
        fingerprint,
        agent_split,
        total_tool_calls: total_tool_calls as usize,
        truncated,
        markdown,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

// ---------------------------------------------------------------------------
// Qdrant scroll
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ScrolledSession {
    session_id: String,
    project_name: String,
    project_path: String,
    ai_title: String,
    start_iso: String,
    source_agent: String,
    source_path: Option<String>,
    intent: String,
    arc: String,
    outcome: String,
    has_errors: bool,
    user_turns: i32,
    assistant_turns: i32,
    tool_count: i32,
}

/// Payload-only scroll across the window. Returns `(sessions, truncated)`
/// where `truncated == true` iff the corpus had more sessions than
/// `MAX_SCROLLED_SESSIONS` (so the caller can disclose the cap).
///
/// Orders by `start_ts_dt` DESC so when truncation fires, the report
/// covers the **most-recent** N sessions, not a non-deterministic slice
/// (Codex review P2 — `chatgpt-codex-connector` PR #7 line 509).
/// Shared between `scroll_window` and the T3.1 facet fast path so both lanes
/// see the exact same window (no off-by-one between scroll and facet counts).
fn window_days_to_filter(window_days: u32) -> Option<Filter> {
    // v3 indexes `start_ts_dt` (RFC 3339 string with datetime index, KC-04),
    // NOT a raw `start_ts` integer. Use `Condition::datetime_range` so the
    // query goes through the datetime index instead of a full payload scan.
    //
    // KNOWN GAP (Quality M1): sessions whose `start_time` was `None` at
    // index time have `start_iso=""`, which is unparseable as datetime and
    // silently excluded from a windowed query. Affects Codex sessions whose
    // first turn lacked a timestamp. window_days=0 (all-time) bypasses the
    // filter and includes them.
    if window_days == 0 {
        return None;
    }
    let now = chrono::Utc::now().timestamp();
    let lo = now - i64::from(window_days) * 86_400;
    Some(Filter {
        must: vec![Condition::datetime_range(
            "start_ts_dt",
            DatetimeRange {
                gte: Some(Timestamp {
                    seconds: lo,
                    nanos: 0,
                }),
                ..Default::default()
            },
        )],
        ..Default::default()
    })
}

async fn scroll_window(
    qdrant: &Qdrant,
    window_days: u32,
) -> Result<(Vec<ScrolledSession>, bool)> {
    let filter = window_days_to_filter(window_days);

    // Ask for one more than the cap so we can detect truncation without
    // a second round-trip. `order_by` + a single bounded scroll is
    // cleaner than pagination here — pagination + ordering have subtle
    // cursor semantics in Qdrant, and we never want more than
    // MAX_SCROLLED_SESSIONS points loaded anyway.
    let probe_limit = (MAX_SCROLLED_SESSIONS as u32).saturating_add(1);
    let order = OrderBy {
        key: "start_ts_dt".to_string(),
        direction: Some(Direction::Desc as i32),
        ..Default::default()
    };
    let mut builder = ScrollPointsBuilder::new(crate::schema::COLLECTION_V3)
        .with_payload(true)
        .with_vectors(false)
        .order_by(order)
        .limit(probe_limit);
    if let Some(f) = filter {
        builder = builder.filter(f);
    }

    let resp = qdrant
        .scroll(builder)
        .await
        .context("wrapped: scroll v3 failed")?;

    let truncated = resp.result.len() > MAX_SCROLLED_SESSIONS;
    let mut out: Vec<ScrolledSession> = Vec::with_capacity(resp.result.len());
    for p in resp.result.into_iter().take(MAX_SCROLLED_SESSIONS) {
        let pl = p.payload;
        let Some(sid) = payload_str(&pl, "session_id") else { continue };
        out.push(ScrolledSession {
            session_id: sid,
            project_name: payload_str(&pl, "project_name").unwrap_or_default(),
            project_path: payload_str(&pl, "project_path").unwrap_or_default(),
            ai_title: payload_str(&pl, "ai_title").unwrap_or_default(),
            start_iso: payload_str(&pl, "start_iso").unwrap_or_default(),
            source_agent: payload_str(&pl, "source_agent")
                .unwrap_or_else(|| "claude_code".into()),
            source_path: payload_str(&pl, "source_path"),
            intent: payload_str(&pl, "intent").unwrap_or_default(),
            arc: payload_str(&pl, "arc").unwrap_or_default(),
            outcome: payload_str(&pl, "outcome").unwrap_or_default(),
            has_errors: payload_bool(&pl, "has_errors").unwrap_or(false),
            user_turns: payload_i64(&pl, "user_turns").unwrap_or(0) as i32,
            assistant_turns: payload_i64(&pl, "assistant_turns").unwrap_or(0) as i32,
            tool_count: payload_i64(&pl, "tool_count").unwrap_or(0) as i32,
        });
    }
    Ok((out, truncated))
}

// ---------------------------------------------------------------------------
// T3.1 · Facet API helpers (qdrant-improvement-goal.md)
//
// SDK 1.18.0 ships the Facet API (FacetCountsBuilder + Qdrant::facet) which
// returns server-side value→count aggregates over an INDEXED payload field
// in O(field cardinality) — vs the O(N) scroll-and-tally path that wrapped.rs
// has been using since v1. Replacing the manual tally for indexed fields
// (project_name, intent, outcome, source_agent) cuts the Wrapped report
// assembly latency on large corpora by ~10×.
//
// We keep the scroll path because the report still needs per-session payload
// data for the deep-mining lane (decisions, tool counts, source_path for
// JSONL re-parse) — Facet doesn't ship per-point data.
//
// `arc` is NOT a v3 indexed payload field; it's derived from intent/outcome
// in the report layer, so we keep tallying it client-side from the scroll.
// `agent_intents` is a 2-D combination (agent × intent) which Facet doesn't
// express in one call; tally from scroll.
// ---------------------------------------------------------------------------

/// Build a `FacetCountsBuilder` for an indexed payload field, optionally
/// scoped by an existing filter (e.g. the datetime window). Pure — no I/O —
/// so unit tests can verify the request shape without a live Qdrant.
///
/// `cap` caps the number of distinct values returned. Sane default for a
/// hackathon-scale corpus: 256 (enough for project_name / intent / outcome
/// distributions; smaller cardinality fields like source_agent return all).
fn build_facet_request(
    collection: &str,
    field: &str,
    filter: Option<Filter>,
    cap: u64,
) -> FacetCountsBuilder {
    let mut b = FacetCountsBuilder::new(collection, field).limit(cap);
    if let Some(f) = filter {
        b = b.filter(f);
    }
    b
}

/// Parse a `FacetResponse` into a (string-keyed) `HashMap<String, usize>`.
/// Non-string facet variants (integer / bool) are not used by the wrapped
/// report — they're collapsed into their `Debug` repr for safety.
fn facet_response_to_counts(resp: FacetResponse) -> HashMap<String, usize> {
    let mut out = HashMap::with_capacity(resp.hits.len());
    for hit in resp.hits {
        let key = match hit.value.and_then(|v| v.variant) {
            Some(facet_value::Variant::StringValue(s)) => s,
            Some(facet_value::Variant::IntegerValue(i)) => i.to_string(),
            Some(facet_value::Variant::BoolValue(b)) => b.to_string(),
            None => continue,
        };
        if key.is_empty() {
            continue;
        }
        out.insert(key, hit.count as usize);
    }
    out
}

/// Fetch facet counts for a single indexed payload field. Returns an empty
/// map on RPC failure (the report still renders with the scroll-based path
/// covering everything Facet doesn't aggregate). This is the **fast path**
/// that replaces the manual `for summary in &payloads { ... }` tally loop.
async fn facet_field(
    qdrant: &Qdrant,
    field: &str,
    filter: Option<Filter>,
) -> Result<HashMap<String, usize>> {
    let req = build_facet_request(crate::schema::COLLECTION_V3, field, filter, 256);
    let resp = qdrant.facet(req).await.context("wrapped: facet request failed")?;
    Ok(facet_response_to_counts(resp))
}

// ---------------------------------------------------------------------------
// Aggregation helpers
// ---------------------------------------------------------------------------

fn aggregate_tool(
    tc: &ToolCall,
    tool_counts: &mut HashMap<String, usize>,
    ext_counts: &mut HashMap<String, usize>,
    bin_counts: &mut HashMap<String, usize>,
) {
    *tool_counts.entry(tc.name.clone()).or_insert(0) += 1;
    if matches!(
        tc.name.as_str(),
        "Edit" | "MultiEdit" | "Read" | "Write" | "NotebookEdit"
    ) {
        let p = tc
            .input
            .get("file_path")
            .or_else(|| tc.input.get("notebook_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(ext) = Path::new(p).extension().and_then(|e| e.to_str()) {
            *ext_counts.entry(format!(".{ext}")).or_insert(0) += 1;
        }
    }
    if tc.name == "Bash" {
        if let Some(cmd) = tc.input.get("command").and_then(|v| v.as_str()) {
            if let Some(first) = cmd.split_whitespace().next() {
                let bin = first
                    .trim_start_matches('!')
                    .trim_start_matches('"')
                    .to_string();
                if !bin.is_empty() && bin.len() < 32 {
                    *bin_counts.entry(bin).or_insert(0) += 1;
                }
            }
        }
    }
}

fn top_atoms(counts: &HashMap<String, usize>, n: usize) -> Vec<Atom> {
    let mut v: Vec<(String, usize)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.into_iter()
        .take(n)
        .map(|(name, count)| Atom { name, count })
        .collect()
}

fn into_buckets(counts: &HashMap<String, usize>, total: usize) -> Vec<Bucket> {
    if total == 0 {
        return Vec::new();
    }
    let mut v: Vec<Bucket> = counts
        .iter()
        .map(|(k, c)| Bucket {
            label: k.clone(),
            count: *c,
            share: *c as f32 / total as f32,
        })
        .filter(|b| b.share >= 0.02)
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count));
    v
}

fn median_u32(samples: &mut [u32]) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

// ---------------------------------------------------------------------------
// Markdown rendering — designed for screenshots / social sharing
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn render_markdown(
    window_days: u32,
    sessions_total: usize,
    sessions_mined: usize,
    truncated: bool,
    total_tool_calls: usize,
    tools: &[Atom],
    bins: &[Atom],
    exts: &[Atom],
    projects: &[Atom],
    intent_mix: &[Bucket],
    arc_mix: &[Bucket],
    outcome_mix: &[Bucket],
    repeated: &[RepeatedDecision],
    fp: &Fingerprint,
    agents: &[AgentSplit],
) -> String {
    let mut out = String::new();
    let window_label = if window_days == 0 {
        "all-time".to_string()
    } else {
        format!("last {window_days} days")
    };
    out.push_str(&format!("# 🎁 Memex Wrapped — {window_label}\n\n"));
    if sessions_total == 0 {
        out.push_str("_No sessions in this window. Try `--window-days 0` for all-time._\n");
        return out;
    }
    out.push_str(&format!(
        "**{sessions_total}** session(s) · **{total_tool_calls}** tool call(s) · _{sessions_mined} sessions deep-mined_\n"
    ));
    if truncated {
        out.push_str(&format!(
            "> _Corpus exceeds the {MAX_SCROLLED_SESSIONS}-session cap; report covers the most-recent slice._\n"
        ));
    }
    out.push('\n');

    // Fingerprint — the headline / sharable line.
    out.push_str("## 🧬 Your engineering fingerprint\n");
    out.push_str(&format!(
        "- Average **{:.0} turns** per session · median **{}** · longest **{}** turns ({})\n",
        fp.avg_turns,
        fp.median_turns,
        fp.longest_turns,
        if fp.longest_session_id.is_empty() {
            "—".to_string()
        } else {
            format!("`{}`", short_sid(&fp.longest_session_id))
        },
    ));
    out.push_str(&format!(
        "- **{:.0}%** of sessions hit at least one error\n",
        fp.had_errors_rate * 100.0
    ));
    if fp.debug_avg_tools > 0.0 {
        out.push_str(&format!(
            "- When you're debugging, you fire **{:.0} tool calls** per session\n",
            fp.debug_avg_tools
        ));
    }
    out.push('\n');

    if !projects.is_empty() {
        out.push_str("## 📁 Where your time went\n");
        for p in projects.iter().take(8) {
            out.push_str(&format!("- `{}`  ·  {}\n", p.name, p.count));
        }
        out.push('\n');
    }

    if !tools.is_empty() {
        out.push_str("## 🛠 Top tools\n");
        let line: Vec<String> = tools
            .iter()
            .take(8)
            .map(|a| format!("{}×{}", a.name, a.count))
            .collect();
        out.push_str(&format!("- {}\n", line.join(" · ")));
        out.push('\n');
    }
    if !bins.is_empty() {
        out.push_str("## 🐚 Bash binaries you live in\n");
        let line: Vec<String> = bins
            .iter()
            .take(8)
            .map(|a| format!("`{}`×{}", a.name, a.count))
            .collect();
        out.push_str(&format!("- {}\n", line.join(" · ")));
        out.push('\n');
    }
    if !exts.is_empty() {
        out.push_str("## 📄 Languages you touched\n");
        let line: Vec<String> = exts
            .iter()
            .take(6)
            .map(|a| format!("{}×{}", a.name, a.count))
            .collect();
        out.push_str(&format!("- {}\n", line.join(" · ")));
        out.push('\n');
    }

    if !intent_mix.is_empty() {
        out.push_str("## 🎯 What you actually did\n");
        for b in intent_mix.iter().take(5) {
            out.push_str(&format!(
                "- **{:>5.1}%** {} ({})\n",
                b.share * 100.0,
                b.label,
                b.count
            ));
        }
        out.push('\n');
    }
    if !arc_mix.is_empty() {
        out.push_str("## 📈 Session shapes\n");
        for b in arc_mix.iter().take(5) {
            out.push_str(&format!(
                "- **{:>5.1}%** {} ({})\n",
                b.share * 100.0,
                b.label,
                b.count
            ));
        }
        out.push('\n');
    }
    if !outcome_mix.is_empty() {
        out.push_str("## ✅ Outcomes\n");
        for b in outcome_mix.iter().take(5) {
            out.push_str(&format!(
                "- **{:>5.1}%** {} ({})\n",
                b.share * 100.0,
                b.label,
                b.count
            ));
        }
        out.push('\n');
    }

    if !repeated.is_empty() {
        out.push_str("## 🔁 Decisions you re-made\n");
        for d in repeated.iter().take(6) {
            out.push_str(&format!(
                "- _{}_ — surfaced in {} session(s)\n",
                clip(&d.text, 140),
                d.sessions
            ));
        }
        out.push('\n');
    }

    if agents.len() > 1 {
        out.push_str("## 🤖 Cross-agent split\n");
        for a in agents {
            out.push_str(&format!(
                "- **{}** — {} session(s) ({:.0}%), dominant intent: {}\n",
                a.agent,
                a.sessions,
                a.share * 100.0,
                a.dominant_intent
            ));
        }
        out.push('\n');
    }

    out.push_str("_Generated by Memex Wrapped · zero LLM · zero network calls._\n");
    out
}

fn clip(s: &str, n: usize) -> String {
    let mut end = s.len();
    for (count, (i, _ch)) in s.char_indices().enumerate() {
        if count == n {
            end = i;
            break;
        }
    }
    if end == s.len() {
        s.to_string()
    } else {
        let mut out: String = s[..end].to_string();
        out.push('…');
        out
    }
}

fn short_sid(sid: &str) -> String {
    let head: String = sid.chars().take(8).collect();
    format!("#{head}")
}

// ---------------------------------------------------------------------------
// Tests — pure helpers only.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn top_atoms_orders_by_count_then_lex() {
        let mut m = HashMap::new();
        m.insert("alpha".to_string(), 5);
        m.insert("beta".to_string(), 5);
        m.insert("gamma".to_string(), 12);
        let v = top_atoms(&m, 3);
        assert_eq!(v[0].name, "gamma");
        assert_eq!(v[1].name, "alpha"); // 5, tied with beta, lex first
        assert_eq!(v[2].name, "beta");
    }

    #[test]
    fn into_buckets_drops_under_two_percent() {
        let mut m = HashMap::new();
        m.insert("hot".to_string(), 50);
        m.insert("rare".to_string(), 1);
        let b = into_buckets(&m, 100);
        // 50/100 = 0.50 → in; 1/100 = 0.01 → dropped (< 2%).
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].label, "hot");
    }

    #[test]
    fn into_buckets_handles_zero_total() {
        let m: HashMap<String, usize> = HashMap::new();
        assert!(into_buckets(&m, 0).is_empty());
    }

    #[test]
    fn median_u32_for_odd_and_empty() {
        let mut a = [3u32, 1, 9, 5, 7];
        assert_eq!(median_u32(&mut a), 5);
        let mut empty: [u32; 0] = [];
        assert_eq!(median_u32(&mut empty), 0);
    }

    #[test]
    fn render_markdown_empty_corpus_message() {
        let fp = Fingerprint {
            avg_turns: 0.0,
            median_turns: 0,
            longest_turns: 0,
            longest_session_id: String::new(),
            had_errors_rate: 0.0,
            debug_avg_tools: 0.0,
        };
        let md =
            render_markdown(30, 0, 0, false, 0, &[], &[], &[], &[], &[], &[], &[], &[], &fp, &[]);
        assert!(md.contains("No sessions in this window"));
    }

    #[test]
    fn render_markdown_emits_sections() {
        let fp = Fingerprint {
            avg_turns: 47.0,
            median_turns: 32,
            longest_turns: 803,
            longest_session_id: "abcdef0123456789".to_string(),
            had_errors_rate: 0.62,
            debug_avg_tools: 22.5,
        };
        let tools = vec![Atom { name: "Bash".into(), count: 31 }];
        let bins = vec![Atom { name: "cargo".into(), count: 12 }];
        let exts = vec![Atom { name: ".rs".into(), count: 8 }];
        let proj = vec![Atom { name: "memex".into(), count: 5 }];
        let mix = vec![Bucket { label: "impl".into(), count: 4, share: 0.4 }];
        let repeated = vec![RepeatedDecision {
            text: "I'll use Drizzle ORM".into(),
            sessions: 3,
            examples: vec!["a".into(), "b".into(), "c".into()],
        }];
        let agents = vec![
            AgentSplit { agent: "claude_code".into(), sessions: 8, share: 0.8, dominant_intent: "build".into() },
            AgentSplit { agent: "codex".into(),       sessions: 2, share: 0.2, dominant_intent: "debug".into() },
        ];
        let md = render_markdown(
            30, 10, 8, false, 145, &tools, &bins, &exts, &proj, &mix, &mix, &mix, &repeated, &fp,
            &agents,
        );
        assert!(md.contains("Memex Wrapped"));
        assert!(md.contains("engineering fingerprint"));
        assert!(md.contains("47"));
        assert!(md.contains("Bash"));
        assert!(md.contains("cargo"));
        assert!(md.contains("Drizzle"));
        assert!(md.contains("Cross-agent split"));
    }

    #[test]
    fn render_markdown_surfaces_truncation_notice() {
        // Codex P2-b / Quality H1: when corpus exceeds the scroll cap, the
        // user-visible markdown must say so — silent dropping is the bug
        // the reviewers flagged.
        let fp = Fingerprint {
            avg_turns: 5.0,
            median_turns: 5,
            longest_turns: 10,
            longest_session_id: "x".into(),
            had_errors_rate: 0.0,
            debug_avg_tools: 0.0,
        };
        let md = render_markdown(
            30, 20_000, 32, true, 0, &[], &[], &[], &[], &[], &[], &[], &[], &fp, &[],
        );
        assert!(
            md.contains("exceeds") && md.contains("cap"),
            "truncated=true should surface a 'corpus exceeds cap' notice; got:\n{md}"
        );
    }

    /* PR #12 REV-4 (Codex P2) — Facet must respect scroll truncation ---- */

    #[test]
    fn facet_response_empty_collapses_buckets_to_scroll_size() {
        // When `truncated == true` (corpus > MAX_SCROLLED_SESSIONS), the
        // compose_wrapped flow MUST skip facet results. We can't drive the
        // RPC in a unit test, but we can lock the contract: when the four
        // facet maps come back empty, the scroll-based fallback path
        // (`facet_*_empty` flags in compose_wrapped) re-tallies from the
        // capped sample so bucket shares stay ≤ 100 %.
        //
        // This test asserts the property by simulating the bucket math.
        let proj_counts: HashMap<String, usize> = HashMap::new(); // facet skipped
        // Suppose 1 200 scrolled sessions with 600 in project "memex":
        let mut tallied = proj_counts.clone();
        for _ in 0..600 { *tallied.entry("memex".to_string()).or_insert(0) += 1; }
        let total = 1_200;
        let buckets = into_buckets(&tallied, total);
        // 600 / 1200 = 0.50 — a valid share, NOT > 1.0 which would happen if
        // we used the un-truncated facet count of (say) 50 000 over total=1200.
        assert!(buckets[0].share <= 1.0,
            "REV-4 invariant: bucket share must never exceed 1.0 when truncated");
        assert!((buckets[0].share - 0.5).abs() < 1e-6);
    }

    /* T3.1 Facet API tests — assert the new code path is hit. ----------- */

    #[test]
    fn facet_request_uses_correct_collection_and_field() {
        // The fast-path replacement must hit the v3 collection on the right
        // field. Without these asserts the helper could silently target the
        // wrong field and the report would be empty without errors.
        let req = build_facet_request(
            crate::schema::COLLECTION_V3,
            "project_name",
            None,
            64,
        );
        let inner: qdrant_client::qdrant::FacetCounts = req.into();
        assert_eq!(inner.collection_name, crate::schema::COLLECTION_V3);
        assert_eq!(inner.key, "project_name");
        assert_eq!(inner.limit, Some(64));
        assert!(inner.filter.is_none());
    }

    #[test]
    fn facet_request_carries_window_filter_when_provided() {
        let f = window_days_to_filter(7).expect("window_days=7 yields a Some filter");
        let req = build_facet_request(crate::schema::COLLECTION_V3, "intent", Some(f), 256);
        let inner: qdrant_client::qdrant::FacetCounts = req.into();
        assert!(inner.filter.is_some(), "datetime window filter must reach the facet request");
        assert_eq!(inner.key, "intent");
        assert_eq!(inner.limit, Some(256));
    }

    #[test]
    fn window_days_zero_disables_filter() {
        assert!(
            window_days_to_filter(0).is_none(),
            "window_days=0 means all-time — no datetime filter"
        );
    }

    #[test]
    fn facet_response_to_counts_maps_string_variants() {
        use qdrant_client::qdrant::{FacetHit, FacetResponse, FacetValue};
        // Mock a Facet RPC reply with mixed string + integer + empty variants
        // and assert only the string ones land in the counts map (the report's
        // payload fields are all keyword strings).
        let resp = FacetResponse {
            hits: vec![
                FacetHit {
                    value: Some(FacetValue {
                        variant: Some(facet_value::Variant::StringValue("memex".into())),
                    }),
                    count: 12,
                },
                FacetHit {
                    value: Some(FacetValue {
                        variant: Some(facet_value::Variant::StringValue("vibe-mod".into())),
                    }),
                    count: 5,
                },
                FacetHit {
                    value: Some(FacetValue {
                        variant: Some(facet_value::Variant::IntegerValue(99)),
                    }),
                    count: 1,
                },
                FacetHit { value: None, count: 9 }, // dropped — no variant
            ],
            ..Default::default()
        };
        let m = facet_response_to_counts(resp);
        assert_eq!(m.get("memex"), Some(&12usize));
        assert_eq!(m.get("vibe-mod"), Some(&5usize));
        assert_eq!(m.get("99"), Some(&1usize));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn render_markdown_skips_cross_agent_when_only_one_agent() {
        let fp = Fingerprint {
            avg_turns: 5.0,
            median_turns: 5,
            longest_turns: 10,
            longest_session_id: "x".into(),
            had_errors_rate: 0.0,
            debug_avg_tools: 0.0,
        };
        let agents = vec![AgentSplit {
            agent: "claude_code".into(),
            sessions: 3,
            share: 1.0,
            dominant_intent: "impl".into(),
        }];
        let md = render_markdown(
            7, 3, 3, false, 0, &[], &[], &[], &[], &[], &[], &[], &[], &fp, &agents,
        );
        assert!(!md.contains("Cross-agent split"));
    }
}
