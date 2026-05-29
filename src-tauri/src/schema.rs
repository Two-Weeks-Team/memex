//! Schema definitions for the **v3** Qdrant collection (`memex_sessions_v3`).
//!
//! Phase 3 (SOTA v3.2) introduces a parallel collection that adds:
//! - **TurboQuant bits2** quantization with rescore + 2.0× oversampling
//! - A `content_late` multivector (MaxSim) — frozen at `m: 0` (no HNSW links)
//!   pending Phase 4 wiring
//! - `path_sparse` + `tool_sparse` sparse vectors with IDF modifier
//! - Tenant payload index on `project_name` (KC-03)
//! - Datetime payload index on `start_ts_dt` (KC-04, RFC 3339 strings)
//! - `schema_version: 3` payload field on every point (KG-03)
//! - `source_agent` keyword payload index (KH-01, v0.4 multi-agent addendum)
//! - Enrich-stage fields (`intent`, `entities`, `outcome`, `arc`, `topic`) reserved
//!   here, populated by P5 enrich.rs
//!
//! The v2 collection (`memex_sessions`, defined in `indexer.rs`) is **frozen**:
//! reads dual-fall-back to v2 via `crud::dual_get_session_payload`, writes go to
//! v3 only.

use std::collections::HashMap;

use qdrant_client::qdrant::{
    quantization_config::Quantization, vectors_config, CreateCollectionBuilder,
    DatetimeIndexParamsBuilder, Distance, FieldType, HnswConfigDiff, KeywordIndexParamsBuilder,
    Modifier, MultiVectorComparator, MultiVectorConfigBuilder, PayloadIndexParams,
    QuantizationSearchParams, QuantizationSearchParamsBuilder, SearchParams, SearchParamsBuilder,
    SparseIndexConfig, SparseVectorConfig, SparseVectorParams, SparseVectorParamsBuilder,
    StrictModeConfigBuilder, TurboQuantBitSize, TurboQuantizationBuilder,
    VectorParams, VectorParamsBuilder, VectorParamsMap, VectorsConfig, WalConfigDiff,
};
use qdrant_client::Payload;
use serde::{Deserialize, Serialize};

/// Name of the v3 Qdrant collection.
pub const COLLECTION_V3: &str = "memex_sessions_v3";

/// Schema version stamped onto every v3 point.
pub const SCHEMA_VERSION_V3: u32 = 3;

/// Embedding dimensionality (BGE-small-en-v1.5 — same as v2).
pub const EMBED_DIM_V3: u64 = 384;

/// Names of the 5 dense vectors carried by every v3 point. Mirrors v2 so the
/// extract logic in `indexer::session_extracts` is reused unchanged.
pub const DENSE_VECTORS: &[&str] = &["content", "tool", "path", "error", "code"];

/// Name of the multivector reserved for late-interaction reranking (P4).
pub const MULTIVECTOR_NAME: &str = "content_late";

/// Names of the two sparse vectors (path + tool, IDF-modified).
pub const SPARSE_VECTORS: &[&str] = &["path_sparse", "tool_sparse"];

// HNSW knobs (per spec §1.1)
const HNSW_M: u64 = 16;
const HNSW_EF_CONSTRUCT: u64 = 100;
const HNSW_PAYLOAD_M: u64 = 16;

// Per-vector HNSW overrides (P5 KC-02).
//
// SPEC NOTE (P5, KC-02): the original P3 config used a single global HNSW
// block. P5 keeps that global block as the FALLBACK and additionally pins
// vector-specific overrides where the cost/benefit is worth the build-time
// memory hit. The cheat sheet:
//
// - `content` is the highest-dimensional semantic surface (full transcript) →
//   denser graph (m=24, ef_construct=200) for better recall.
// - `code` is similar but smaller corpus (per-session code blocks) → mid
//   density (m=20, ef_construct=150).
// - `error` is a small, high-signal surface → moderate density (m=16/100).
// - `tool` and `path` are short, repetitive token surfaces where the graph
//   doesn't need to be dense — m=12/64 is enough for cosine accuracy at half
//   the build cost.
// - `content_late` (multivector) keeps m=0 (no HNSW links) so it stays
//   rerank-only per the P3/P4 design.
const HNSW_CONTENT_M: u64 = 24;
const HNSW_CONTENT_EF_CONSTRUCT: u64 = 200;
const HNSW_TOOL_PATH_M: u64 = 12;
const HNSW_TOOL_PATH_EF_CONSTRUCT: u64 = 64;
const HNSW_ERROR_M: u64 = 16;
const HNSW_ERROR_EF_CONSTRUCT: u64 = 100;
const HNSW_CODE_M: u64 = 20;
const HNSW_CODE_EF_CONSTRUCT: u64 = 150;

// Strict mode (per spec §1.1)
const STRICT_MAX_RESIDENT_MEMORY_PERCENT: u32 = 85;
// P5 KC-06 — cap server-accepted limit to 100 so a runaway client can't
// request 100k points and OOM the embedded Qdrant.
const STRICT_MAX_QUERY_LIMIT: u32 = 100;

// WAL (per spec §1.1)
const WAL_CAPACITY_MB: u64 = 32;

// Quantization search params (per spec §2.2 — applied to every search call)
const QUANTIZATION_OVERSAMPLING: f64 = 2.0;

// ---------------------------------------------------------------------------
// Collection builder
// ---------------------------------------------------------------------------

/// Build the full `CreateCollection` request body for the v3 collection.
///
/// Idempotency lives one level up in `crud::ensure_collection_v3` — this
/// function just describes the target shape.
pub fn build_v3_create_collection() -> CreateCollectionBuilder {
    // 5 dense named vectors (cosine, 384-d) — same names/dims as v2.
    //
    // SPEC NOTE (P5 KC-02): each vector now carries an HNSW override matched
    // to its semantic role (see HNSW_* constants above). The qdrant-client
    // 1.18 builder accepts the override via `.hnsw_config(HnswConfigDiff {..})`
    // on each `VectorParamsBuilder`. The global `hnsw_config` on
    // `CreateCollectionBuilder` (below) is still set as the fallback for any
    // future vector that doesn't carry its own override.
    let mut params_map: HashMap<String, VectorParams> = HashMap::new();
    for name in DENSE_VECTORS {
        let (m, ef) = match *name {
            "content" => (HNSW_CONTENT_M, HNSW_CONTENT_EF_CONSTRUCT),
            "tool" | "path" => (HNSW_TOOL_PATH_M, HNSW_TOOL_PATH_EF_CONSTRUCT),
            "error" => (HNSW_ERROR_M, HNSW_ERROR_EF_CONSTRUCT),
            "code" => (HNSW_CODE_M, HNSW_CODE_EF_CONSTRUCT),
            // Defensive default — should never hit because DENSE_VECTORS is
            // exhaustive and matches the cases above.
            _ => (HNSW_M, HNSW_EF_CONSTRUCT),
        };
        params_map.insert(
            (*name).to_string(),
            VectorParamsBuilder::new(EMBED_DIM_V3, Distance::Cosine)
                .hnsw_config(HnswConfigDiff {
                    m: Some(m),
                    ef_construct: Some(ef),
                    ..Default::default()
                })
                .build(),
        );
    }
    // Multivector — frozen at m:0 (no HNSW links) so it doesn't compete with
    // dense HNSW for memory until P4 wires the rerank path.
    let multi_vec_params: VectorParams = VectorParamsBuilder::new(EMBED_DIM_V3, Distance::Cosine)
        .multivector_config(
            // SPEC NOTE (P3, AC-3.1.1): qdrant-client 1.18 exposes
            // `MultiVectorConfigBuilder::new(comparator)` rather than the
            // `default().comparator(..).build()` shape some docs hint at.
            MultiVectorConfigBuilder::new(MultiVectorComparator::MaxSim).build(),
        )
        .hnsw_config(HnswConfigDiff {
            m: Some(0),
            ..Default::default()
        })
        .build();
    params_map.insert(MULTIVECTOR_NAME.to_string(), multi_vec_params);

    let vectors_cfg: VectorsConfig =
        vectors_config::Config::ParamsMap(VectorParamsMap { map: params_map }).into();

    // 2 sparse vectors with IDF modifier.
    let mut sparse_map: HashMap<String, SparseVectorParams> = HashMap::new();
    for name in SPARSE_VECTORS {
        sparse_map.insert(
            (*name).to_string(),
            SparseVectorParamsBuilder::default()
                .modifier(Modifier::Idf)
                .index(SparseIndexConfig {
                    full_scan_threshold: None,
                    on_disk: None,
                    datatype: None,
                })
                .build(),
        );
    }
    let sparse_cfg = SparseVectorConfig { map: sparse_map };

    // Global HNSW config.
    let hnsw = HnswConfigDiff {
        m: Some(HNSW_M),
        ef_construct: Some(HNSW_EF_CONSTRUCT),
        payload_m: Some(HNSW_PAYLOAD_M),
        ..Default::default()
    };

    // Quantization mode (Issue #13).
    //
    // SPEC NOTE (P3, KC-01): `CreateCollectionBuilder.quantization_config`
    // accepts `Into<quantization_config::Quantization>` — the inner *oneof*
    // variant, not the wrapper `QuantizationConfig`. The server still stores
    // a `QuantizationConfig { quantization: Some(Turboquant(...)) }` on the
    // collection — verified in `it_quantization_present_in_collection_info`.
    //
    // Issue #13 made this runtime-configurable via `MEMEX_QUANT_MODE`. The
    // default (env unset / unrecognised) is **the same** TurboQuant bits-2 +
    // always_ram production setting we had pre-#13 — no behaviour change for
    // existing deployments. The new env var is used by `benches/quant_sweep.rs`
    // and by anyone running `cargo bench --bench quant_sweep` to compare
    // recall / latency / disk-size across configs on their own corpus.
    let quantization_inner: Option<Quantization> = QuantMode::from_env().to_inner_quantization();

    // Strict mode — KC-06 (P5) adds `max_query_limit` on top of P3's
    // `max_resident_memory_percent`. Both are conservative: 85% memory cap
    // is what Qdrant docs call the "embedded-friendly" ceiling, and a query
    // limit of 100 is well above the highest `per_vector_limit` we ever use
    // (60 in `lens_search`) but blocks ridiculous client requests.
    let strict_mode = StrictModeConfigBuilder::default()
        .max_resident_memory_percent(STRICT_MAX_RESIDENT_MEMORY_PERCENT)
        .max_query_limit(STRICT_MAX_QUERY_LIMIT)
        .build();

    // WAL — small capacity is fine for a single-machine indexer.
    let wal = WalConfigDiff {
        wal_capacity_mb: Some(WAL_CAPACITY_MB),
        ..Default::default()
    };

    let mut builder = CreateCollectionBuilder::new(COLLECTION_V3)
        .vectors_config(vectors_cfg)
        .sparse_vectors_config(sparse_cfg)
        .hnsw_config(hnsw)
        .strict_mode_config(strict_mode)
        .wal_config(wal)
        .timeout(60);
    // F32None (no quantization at all) is selected by omitting the field.
    // Any other QuantMode wraps the inner oneof variant.
    if let Some(q) = quantization_inner {
        builder = builder.quantization_config(q);
    }
    builder
}

// ---------------------------------------------------------------------------
// Payload index map
// ---------------------------------------------------------------------------

/// Returns the ordered list of payload indexes the v3 collection must carry.
///
/// The third tuple element is the optional **typed** params (e.g.,
/// `KeywordIndexParams { is_tenant: true }` for the tenant index on
/// `project_name`, or `DatetimeIndexParams` for `start_ts_dt`). `None` =
/// default params for the given `FieldType`.
pub fn v3_payload_indexes() -> Vec<(&'static str, FieldType, Option<PayloadIndexParams>)> {
    use qdrant_client::qdrant::payload_index_params::IndexParams;

    let tenant_kw: PayloadIndexParams = PayloadIndexParams {
        index_params: Some(IndexParams::KeywordIndexParams(
            KeywordIndexParamsBuilder::default()
                .is_tenant(true)
                .build(),
        )),
    };
    let datetime_default: PayloadIndexParams = PayloadIndexParams {
        index_params: Some(IndexParams::DatetimeIndexParams(
            DatetimeIndexParamsBuilder::default().build(),
        )),
    };

    vec![
        ("project_name", FieldType::Keyword, Some(tenant_kw)),
        ("project_path", FieldType::Keyword, None),
        ("git_branch", FieldType::Keyword, None),
        ("ai_title", FieldType::Text, None),
        // Issue #14 — multi-value identifier tokens with their own Text index.
        // Each Vec element is treated as an independent token by Qdrant's
        // Text index, so the standard `word` tokenizer is correct here once
        // we've split client-side.
        ("ai_title_tokens", FieldType::Text, None),
        ("start_ts_dt", FieldType::Datetime, Some(datetime_default)),
        ("has_errors", FieldType::Bool, None),
        ("schema_version", FieldType::Integer, None),
        ("intent", FieldType::Keyword, None),
        ("outcome", FieldType::Keyword, None),
        ("source_agent", FieldType::Keyword, None), // KH-01
    ]
}

// ---------------------------------------------------------------------------
// Issue #13 — runtime-configurable quantization mode (MEMEX_QUANT_MODE env)
// ---------------------------------------------------------------------------
//
// Used by `build_v3_create_collection()` above + the Criterion benchmark in
// `benches/quant_sweep.rs`. The default (env unset / unrecognised) is the
// same TurboQuant bits-2 setting we shipped pre-#13.
//
// Recognised values:
//   - "f32" / "none"  → no quantization at all (f32 baseline)
//   - "tq-bits1"      → TurboQuant 1-bit (more compression, lower recall)
//   - "tq-bits2"      → TurboQuant 2-bit (default — current production)
//
// Scalar / Binary quantization variants are intentionally not included in
// this first round — they would require additional builder plumbing and the
// 3 modes above already span the interesting recall vs. disk-size space.
// Adding them is a 30-line follow-up if a sweep needs more granularity.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantMode {
    F32None,
    TqBits1,
    TqBits2,
}

impl QuantMode {
    /// Read the mode from `MEMEX_QUANT_MODE`. Unset / unrecognised → TqBits2
    /// (the pre-#13 hardcoded production default).
    pub fn from_env() -> Self {
        match std::env::var("MEMEX_QUANT_MODE")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("f32") | Some("none") => Self::F32None,
            Some("tq-bits1") | Some("tq_bits1") | Some("turboquant_bits1") => Self::TqBits1,
            // Default branch covers "tq-bits2", "tq_bits2", unset, unknown.
            _ => Self::TqBits2,
        }
    }

    /// The kebab-case name used in env values and reports.
    pub fn as_name(self) -> &'static str {
        match self {
            Self::F32None => "f32",
            Self::TqBits1 => "tq-bits1",
            Self::TqBits2 => "tq-bits2",
        }
    }

    /// Convert into the inner Qdrant `Quantization` oneof variant.
    /// `None` means "no quantization at all" (f32 baseline) — handled at the
    /// caller by omitting the builder field entirely.
    pub fn to_inner_quantization(self) -> Option<Quantization> {
        match self {
            Self::F32None => None,
            Self::TqBits1 => Some(Quantization::Turboquant(
                TurboQuantizationBuilder::default()
                    .bits(TurboQuantBitSize::Bits1)
                    .always_ram(true)
                    .build(),
            )),
            Self::TqBits2 => Some(Quantization::Turboquant(
                TurboQuantizationBuilder::default()
                    .bits(TurboQuantBitSize::Bits2)
                    .always_ram(true)
                    .build(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Quantization search params (applied to every read-side query)
// ---------------------------------------------------------------------------

/// Build the `QuantizationSearchParams` that every v3 search must include
/// (`ignore=false`, `rescore=true`, `oversampling=2.0` per spec §2.2 / KC-01b).
pub fn quantization_search_params() -> QuantizationSearchParams {
    QuantizationSearchParamsBuilder::default()
        .ignore(false)
        .rescore(true)
        .oversampling(QUANTIZATION_OVERSAMPLING)
        .build()
}

/// Build the full `SearchParams` wrapper (typed) — usable for the proto-level
/// `SearchPoints` request. `QueryPointsBuilder.params(...)` accepts this.
pub fn search_params_with_quantization() -> SearchParams {
    SearchParamsBuilder::default()
        .quantization(quantization_search_params())
        .build()
}

// ---------------------------------------------------------------------------
// Source agent inference (KH-01)
// ---------------------------------------------------------------------------

/// Static path-based inference of source agent.
///
/// SPEC NOTE (P3, KH-01): the spec text says `crate::sec::SandboxRoot::detect_agent(path)`
/// but `detect_agent` is an instance method that requires constructing a
/// `SandboxRoot::from_env()` (and canonicalize-ing the path on disk). For
/// migration paths where the source file may no longer exist (or test fixtures
/// pointing at non-existent paths), we fall back to string-matching the
/// canonical root segments. Matches what `sec::SourceAgent::as_str()` returns.
pub fn infer_source_agent(source_path: &str) -> &'static str {
    if source_path.contains("/.codex/sessions/") {
        "codex"
    } else {
        // Default — covers `~/.claude/projects/...` and any path we can't
        // confidently classify as Codex. AC-3.1.8: pre-branch v2 sessions are
        // all claude_code, so this default keeps migration lossless.
        "claude_code"
    }
}

// ---------------------------------------------------------------------------
// Issue #14 — identifier-aware tokenization for `ai_title`
// ---------------------------------------------------------------------------
//
// Qdrant 1.18 ships 4 builtin text tokenizers (`word` / `whitespace` /
// `prefix` / `multilingual`); none of them split identifier-shaped titles
// like `getUserData refactor` or `fix_dns_resolver bug` at the camel/snake
// boundaries — so a search for `user` does not match a session titled
// `getUserData refactor`.
//
// We solve this client-side: at index time we expand the title into a
// `Vec<String>` of constituent tokens stored in a second payload field
// `ai_title_tokens`, which carries its own `Text` index. Qdrant's text
// index treats each array element as an independent token (see
// https://qdrant.tech/documentation/concepts/payload/), so once split
// client-side the `word` tokenizer is correct.
//
// Industry precedent for this pattern: Elasticsearch's
// `word_delimiter_graph` token filter, GitHub Code Search, Sourcegraph
// zoekt — all do the splitting at indexing time, not at query time.

/// Split an `ai_title`-shaped string into its constituent identifier tokens.
///
/// Includes the original whitespace-separated words, the camelCase split,
/// and the snake_case split. Tokens of length 1 and empty tokens are
/// dropped (they only add noise to the text index).
///
/// Examples:
/// - `"getUserData refactor"` → `["getUserData", "get", "User", "Data", "refactor"]`
/// - `"fix_dns_resolver bug"` → `["fix_dns_resolver", "fix", "dns", "resolver", "bug"]`
/// - `"PowerShot 4G"` → `["PowerShot", "Power", "Shot", "4G"]`
pub fn identifier_tokens(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in s.split_whitespace() {
        // Preserve the original token unchanged.
        out.push(word.to_string());
        // Snake/kebab split first; for words without `_`/`-` this returns
        // a single piece equal to `word` itself.
        for piece in word.split(|c: char| c == '_' || c == '-') {
            if piece.is_empty() {
                continue;
            }
            // Don't re-push the original `word` if it had no separator.
            if piece != word {
                out.push(piece.to_string());
            }
            // camelCase / PascalCase split applies to each snake piece.
            extend_camel_split(&mut out, piece);
        }
    }
    // Drop noise tokens (length 0 or 1).
    out.retain(|t| t.chars().count() > 1);
    // Deduplicate while preserving first-occurrence order.
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(t.clone()));
    out
}

/// Push the camelCase / PascalCase split of `word` into `out`.
/// `getUserData` → push `get`, `User`, `Data`. ASCII fast path; non-ASCII
/// characters break the boundary (each non-ASCII char starts a new token).
///
/// Does NOT push the original `word` itself — caller is responsible for
/// deciding whether to preserve the original token alongside the split pieces.
fn extend_camel_split(out: &mut Vec<String>, word: &str) {
    let mut current = String::new();
    let mut prev_is_lower = false;
    let mut piece_count = 0;
    let mut pieces: Vec<String> = Vec::new();
    for ch in word.chars() {
        let is_upper = ch.is_uppercase();
        // Boundary: previous was lowercase and current is uppercase
        //   (`getUser` → split between `get` and `User`).
        if is_upper && prev_is_lower && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
            piece_count += 1;
        }
        current.push(ch);
        prev_is_lower = ch.is_lowercase();
    }
    if !current.is_empty() {
        pieces.push(current);
        piece_count += 1;
    }
    // Only emit the split if it actually split — a single-piece "split" is
    // equal to the original `word`, which the caller already pushed.
    if piece_count > 1 {
        out.extend(pieces);
    }
}

// ---------------------------------------------------------------------------
// V3Payload — typed payload struct
// ---------------------------------------------------------------------------

/// Typed v3 payload. Construct via `V3Payload::from_v2_payload` when migrating
/// an existing v2 point, or via `V3Payload::from_session_fields` when fresh-
/// indexing a Session.
///
/// All enrich-stage fields (`intent`, `entities`, `outcome`, `arc`, `topic`)
/// are reserved here at `None`/empty so the JSON shape is stable from day one
/// of v3. P5's enrich.rs will fill them later via `crud::conditional_update_payload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V3Payload {
    pub session_id: String,
    pub source_path: String,
    pub project_name: String,
    pub project_path: String,
    pub git_branch: String,
    pub ai_title: String,
    /// Issue #14 — identifier-aware token expansion of `ai_title`, indexed
    /// with its own `Text` payload index for camelCase / snake_case search.
    /// Always emitted (empty `Vec` for blank titles); the upsert path fills
    /// this via `identifier_tokens(&ai_title)`.
    #[serde(default)]
    pub ai_title_tokens: Vec<String>,
    pub claude_version: String,
    pub start_iso: String,
    pub end_iso: String,
    /// New in v3: datetime-indexed copy of `start_iso` (kept for clarity;
    /// the indexed field uses the same RFC 3339 string).
    pub start_ts_dt: String,
    pub end_ts_dt: String,
    pub user_turns: i64,
    pub assistant_turns: i64,
    pub tool_count: i64,
    pub has_errors: bool,
    pub schema_version: u32,
    /// KH-01 — `"claude_code"` or `"codex"`.
    pub source_agent: String,
    /// Enrich-stage fields (P5 fills in).
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub arc: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
}

impl V3Payload {
    /// Construct from individual fields. Used by the v3 upsert path in
    /// indexer.rs as well as by tests.
    #[allow(clippy::too_many_arguments)]
    pub fn from_session_fields(
        session_id: String,
        source_path: String,
        project_name: String,
        project_path: String,
        git_branch: String,
        ai_title: String,
        claude_version: String,
        start_iso: String,
        end_iso: String,
        user_turns: i64,
        assistant_turns: i64,
        tool_count: i64,
        has_errors: bool,
    ) -> Self {
        let source_agent = infer_source_agent(&source_path).to_string();
        // Issue #14 — identifier-aware token expansion runs at index time.
        // Empty for blank titles; never `None` so the field is always present
        // on every v3 point and the text index has a uniform shape.
        let ai_title_tokens = identifier_tokens(&ai_title);
        Self {
            start_ts_dt: start_iso.clone(),
            end_ts_dt: end_iso.clone(),
            session_id,
            source_path,
            project_name,
            project_path,
            git_branch,
            ai_title,
            ai_title_tokens,
            claude_version,
            start_iso,
            end_iso,
            user_turns,
            assistant_turns,
            tool_count,
            has_errors,
            schema_version: SCHEMA_VERSION_V3,
            source_agent,
            intent: None,
            entities: Vec::new(),
            outcome: None,
            arc: None,
            topic: None,
        }
    }

    /// Lift a v2 payload (the `HashMap<String, qdrant_client::qdrant::Value>`
    /// returned by `Qdrant.get_points(...)`) into a typed v3 payload. Used by
    /// the migrate path.
    pub fn from_v2_payload(
        v2: &HashMap<String, qdrant_client::qdrant::Value>,
    ) -> anyhow::Result<Self> {
        use qdrant_client::qdrant::value::Kind;
        fn s(p: &HashMap<String, qdrant_client::qdrant::Value>, key: &str) -> String {
            p.get(key)
                .and_then(|v| v.kind.as_ref())
                .and_then(|k| match k {
                    Kind::StringValue(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        }
        fn i(p: &HashMap<String, qdrant_client::qdrant::Value>, key: &str) -> i64 {
            p.get(key)
                .and_then(|v| v.kind.as_ref())
                .and_then(|k| match k {
                    Kind::IntegerValue(i) => Some(*i),
                    _ => None,
                })
                .unwrap_or(0)
        }
        fn b(p: &HashMap<String, qdrant_client::qdrant::Value>, key: &str) -> bool {
            p.get(key)
                .and_then(|v| v.kind.as_ref())
                .and_then(|k| match k {
                    Kind::BoolValue(b) => Some(*b),
                    _ => None,
                })
                .unwrap_or(false)
        }
        let session_id = s(v2, "session_id");
        if session_id.is_empty() {
            anyhow::bail!("v2 payload missing session_id");
        }
        let source_path = s(v2, "source_path");
        let start_iso = s(v2, "start_iso");
        let end_iso = s(v2, "end_iso");
        Ok(Self::from_session_fields(
            session_id,
            source_path,
            s(v2, "project_name"),
            s(v2, "project_path"),
            s(v2, "git_branch"),
            s(v2, "ai_title"),
            s(v2, "claude_version"),
            start_iso,
            end_iso,
            i(v2, "user_turns"),
            i(v2, "assistant_turns"),
            i(v2, "tool_count"),
            b(v2, "has_errors"),
        ))
    }

    /// Convert into a Qdrant `Payload` (the wire format).
    pub fn to_qdrant_payload(&self) -> Payload {
        // serde_json::to_value gives a tidy JSON form we can hand directly to
        // `Payload::try_from(serde_json::Value)`.
        let v = serde_json::to_value(self).expect("V3Payload is JSON-serializable");
        Payload::try_from(v).expect("payload conversion from V3Payload")
    }
}

// ---------------------------------------------------------------------------
// Tests (§1, §2, §3 from tests.md)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qdrant_client::qdrant::payload_index_params::IndexParams;
    use qdrant_client::qdrant::vectors_config::Config as VConfig;

    // ----- §1 schema collection-shape -----

    fn vectors_param_map_v3() -> HashMap<String, VectorParams> {
        let req = build_v3_create_collection().build();
        let cfg = req.vectors_config.expect("vectors_config set").config;
        match cfg.expect("inner config") {
            VConfig::ParamsMap(m) => m.map,
            _ => panic!("v3 should use ParamsMap, not single Params"),
        }
    }

    #[test]
    fn t_collection_config_has_all_dense() {
        // AC-3.1.1 — 5 dense, all 384-d cosine.
        let map = vectors_param_map_v3();
        for name in DENSE_VECTORS {
            let vp = map.get(*name).unwrap_or_else(|| {
                panic!("v3 collection missing dense vector `{name}`");
            });
            assert_eq!(vp.size, EMBED_DIM_V3, "{name} dim mismatch");
            assert_eq!(
                vp.distance,
                Distance::Cosine as i32,
                "{name} distance mismatch"
            );
            // Multivector config should be absent for plain dense vectors.
            assert!(
                vp.multivector_config.is_none(),
                "dense vector {name} should not be multivector"
            );
        }
    }

    #[test]
    fn t_collection_config_per_vector_hnsw_overrides() {
        // P5 KC-02 — each dense vector carries its own (m, ef_construct).
        let map = vectors_param_map_v3();
        let cases: &[(&str, u64, u64)] = &[
            ("content", HNSW_CONTENT_M, HNSW_CONTENT_EF_CONSTRUCT),
            ("tool", HNSW_TOOL_PATH_M, HNSW_TOOL_PATH_EF_CONSTRUCT),
            ("path", HNSW_TOOL_PATH_M, HNSW_TOOL_PATH_EF_CONSTRUCT),
            ("error", HNSW_ERROR_M, HNSW_ERROR_EF_CONSTRUCT),
            ("code", HNSW_CODE_M, HNSW_CODE_EF_CONSTRUCT),
        ];
        for (name, want_m, want_ef) in cases {
            let vp = map.get(*name).unwrap();
            let hnsw = vp
                .hnsw_config
                .as_ref()
                .unwrap_or_else(|| panic!("vector {name} missing per-vector hnsw_config"));
            assert_eq!(
                hnsw.m,
                Some(*want_m),
                "vector {name} m mismatch (got {:?})",
                hnsw.m
            );
            assert_eq!(
                hnsw.ef_construct,
                Some(*want_ef),
                "vector {name} ef_construct mismatch"
            );
        }
    }

    #[test]
    fn t_collection_config_has_multivec() {
        // AC-3.1.1 — content_late: MaxSim, m=0.
        let map = vectors_param_map_v3();
        let mv = map
            .get(MULTIVECTOR_NAME)
            .expect("content_late multivector missing");
        let mvc = mv.multivector_config.expect("multivector_config set");
        assert_eq!(mvc.comparator, MultiVectorComparator::MaxSim as i32);
        let hnsw = mv.hnsw_config.expect("multivector hnsw_config set");
        assert_eq!(hnsw.m, Some(0), "content_late must be m=0 (no HNSW links)");
    }

    #[test]
    fn t_collection_config_has_sparse() {
        // AC-3.1.1 — path_sparse + tool_sparse with IDF modifier.
        let req = build_v3_create_collection().build();
        let sparse = req
            .sparse_vectors_config
            .expect("sparse_vectors_config set");
        for name in SPARSE_VECTORS {
            let sp = sparse
                .map
                .get(*name)
                .unwrap_or_else(|| panic!("sparse vector `{name}` missing"));
            assert_eq!(
                sp.modifier,
                Some(Modifier::Idf as i32),
                "{name} should use IDF modifier"
            );
        }
    }

    #[test]
    fn t_collection_config_has_turbo_bits2() {
        // AC-3.2.1 — TurboQuant with bits=Bits2, always_ram=true.
        let req = build_v3_create_collection().build();
        let qc = req
            .quantization_config
            .expect("quantization_config set on v3");
        let inner = qc.quantization.expect("quantization variant set");
        match inner {
            Quantization::Turboquant(turbo) => {
                assert_eq!(
                    turbo.bits,
                    Some(TurboQuantBitSize::Bits2 as i32),
                    "must be Bits2"
                );
                assert_eq!(turbo.always_ram, Some(true), "must be always_ram=true");
            }
            other => panic!("expected Turboquant variant, got {other:?}"),
        }
    }

    #[test]
    fn t_collection_config_strict_mode() {
        // P3 — strict_mode_config.max_resident_memory_percent == 85.
        // P5 KC-06 — strict_mode_config.max_query_limit == 100 (new).
        let req = build_v3_create_collection().build();
        let strict = req.strict_mode_config.expect("strict_mode set on v3");
        assert_eq!(
            strict.max_resident_memory_percent,
            Some(STRICT_MAX_RESIDENT_MEMORY_PERCENT)
        );
        assert_eq!(
            strict.max_query_limit,
            Some(STRICT_MAX_QUERY_LIMIT),
            "KC-06 — strict_mode must cap max_query_limit"
        );
    }

    #[test]
    fn t_collection_config_hnsw_and_wal() {
        // Sanity — m=16, ef_construct=100, payload_m=16, wal_capacity_mb=32.
        let req = build_v3_create_collection().build();
        let hnsw = req.hnsw_config.expect("hnsw_config set");
        assert_eq!(hnsw.m, Some(HNSW_M));
        assert_eq!(hnsw.ef_construct, Some(HNSW_EF_CONSTRUCT));
        assert_eq!(hnsw.payload_m, Some(HNSW_PAYLOAD_M));
        let wal = req.wal_config.expect("wal_config set");
        assert_eq!(wal.wal_capacity_mb, Some(WAL_CAPACITY_MB));
    }

    // ----- §1 payload-index shape -----

    fn idx<'a>(
        name: &str,
        all: &'a [(&'static str, FieldType, Option<PayloadIndexParams>)],
    ) -> &'a (&'static str, FieldType, Option<PayloadIndexParams>) {
        all.iter()
            .find(|(n, ..)| *n == name)
            .unwrap_or_else(|| panic!("payload index `{name}` missing"))
    }

    #[test]
    fn t_payload_index_tenant_present() {
        // AC-3.3.1 — project_name with is_tenant=true.
        let indexes = v3_payload_indexes();
        let (_, ftype, params) = idx("project_name", &indexes);
        assert_eq!(*ftype, FieldType::Keyword);
        let params = params.as_ref().expect("project_name params present");
        match params.index_params.as_ref().expect("inner index_params") {
            IndexParams::KeywordIndexParams(k) => {
                assert_eq!(k.is_tenant, Some(true));
            }
            other => panic!("expected KeywordIndexParams, got {other:?}"),
        }
    }

    #[test]
    fn t_payload_index_datetime_present() {
        // AC-3.4.1 — start_ts_dt FieldType::Datetime.
        let indexes = v3_payload_indexes();
        let (_, ftype, params) = idx("start_ts_dt", &indexes);
        assert_eq!(*ftype, FieldType::Datetime);
        let params = params.as_ref().expect("start_ts_dt params present");
        assert!(matches!(
            params.index_params.as_ref().expect("inner index_params"),
            IndexParams::DatetimeIndexParams(_)
        ));
    }

    #[test]
    fn t_payload_index_schema_version() {
        // AC-3.1.4 — schema_version FieldType::Integer.
        let indexes = v3_payload_indexes();
        let (_, ftype, _) = idx("schema_version", &indexes);
        assert_eq!(*ftype, FieldType::Integer);
    }

    #[test]
    fn t_payload_index_source_agent_keyword() {
        // AC-3.1.7 (KH-01) — source_agent FieldType::Keyword.
        let indexes = v3_payload_indexes();
        let (_, ftype, _) = idx("source_agent", &indexes);
        assert_eq!(*ftype, FieldType::Keyword);
    }

    #[test]
    fn t_payload_index_count_is_eleven() {
        // Sanity — was 10 pre-Issue#14. Issue #14 added `ai_title_tokens`
        // (identifier-aware text index for camelCase/snake_case search).
        let indexes = v3_payload_indexes();
        assert_eq!(indexes.len(), 11);
        // And specifically: the new ai_title_tokens index is Text-typed.
        let (_, ftype, _) = idx("ai_title_tokens", &indexes);
        assert_eq!(
            *ftype,
            FieldType::Text,
            "ai_title_tokens must use the Text index for token matching",
        );
    }

    // ----- §2 payload v3 shape -----

    fn sample_v3_payload() -> V3Payload {
        V3Payload::from_session_fields(
            "df1906d2-aaaa-bbbb-cccc-dddddddddddd".to_string(),
            "/Users/x/.claude/projects/-Users-x-memex/df1906d2.jsonl".to_string(),
            "memex".to_string(),
            "/Users/x/projects/memex".to_string(),
            "main".to_string(),
            "(test title)".to_string(),
            "2.1.143".to_string(),
            "2026-05-17T09:15:18.335Z".to_string(),
            "2026-05-17T10:48:02.000Z".to_string(),
            232,
            403,
            220,
            true,
        )
    }

    #[test]
    fn t_payload_v3_serializes_schema_version() {
        // AC-3.1.4 — JSON contains "schema_version": 3.
        let p = sample_v3_payload();
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["schema_version"], serde_json::json!(3));
    }

    #[test]
    fn t_payload_v3_includes_enrich_fields_null() {
        let p = sample_v3_payload();
        let v = serde_json::to_value(&p).unwrap();
        assert!(v["intent"].is_null());
        assert!(v["outcome"].is_null());
        assert!(v["arc"].is_null());
        assert!(v["topic"].is_null());
        // entities is an empty array, not null.
        assert!(v["entities"].is_array());
        assert_eq!(v["entities"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn t_payload_v3_includes_dt_field() {
        // AC-3.4.1 — start_ts_dt is ISO 8601 (RFC 3339).
        let p = sample_v3_payload();
        let v = serde_json::to_value(&p).unwrap();
        let s = v["start_ts_dt"].as_str().unwrap();
        assert!(
            s.contains('T') && (s.ends_with('Z') || s.contains('+') || s.contains('-')),
            "start_ts_dt `{s}` doesn't look like RFC 3339"
        );
    }

    #[test]
    fn t_payload_v3_source_agent_claude() {
        // AC-3.1.6 — claude path → "claude_code".
        let p = V3Payload::from_session_fields(
            "sid".into(),
            "/home/u/.claude/projects/-u-foo/x.jsonl".into(),
            "foo".into(),
            "/x".into(),
            "main".into(),
            "t".into(),
            "v".into(),
            "2026-05-17T09:15:18.335Z".into(),
            "2026-05-17T10:48:02.000Z".into(),
            0,
            0,
            0,
            false,
        );
        assert_eq!(p.source_agent, "claude_code");
    }

    #[test]
    fn t_payload_v3_source_agent_codex() {
        // AC-3.1.6 — codex path → "codex".
        let p = V3Payload::from_session_fields(
            "sid".into(),
            "/home/u/.codex/sessions/2026/05/18/rollout-x.jsonl".into(),
            "foo".into(),
            "/x".into(),
            "main".into(),
            "t".into(),
            "v".into(),
            "2026-05-17T09:15:18.335Z".into(),
            "2026-05-17T10:48:02.000Z".into(),
            0,
            0,
            0,
            false,
        );
        assert_eq!(p.source_agent, "codex");
    }

    // ----- §3 quantization search params -----

    #[test]
    fn t_quantization_query_params_default() {
        // AC-3.2.2 — rescore=true, oversampling=2.0.
        let qp = quantization_search_params();
        assert_eq!(qp.rescore, Some(true));
        assert_eq!(qp.oversampling, Some(QUANTIZATION_OVERSAMPLING));
    }

    #[test]
    fn t_quantization_ignore_false_default() {
        // AC-3.2.2 — ignore=false (explicit, not server default).
        let qp = quantization_search_params();
        assert_eq!(qp.ignore, Some(false));
    }

    #[test]
    fn t_search_params_wrapper_has_quantization() {
        // The SearchParams shim carries the quantization block.
        let sp = search_params_with_quantization();
        let qp = sp.quantization.expect("quantization wrapped into SearchParams");
        assert_eq!(qp.rescore, Some(true));
    }

    // ----- v2 → v3 lift -----

    #[test]
    fn t_from_v2_payload_default_agent() {
        // KH-01 — pre-branch v2 payloads (no source_agent) lift to claude_code.
        use qdrant_client::qdrant::value::Kind;
        use qdrant_client::qdrant::Value;
        let mut v2 = HashMap::new();
        v2.insert(
            "session_id".into(),
            Value {
                kind: Some(Kind::StringValue("abc".into())),
            },
        );
        v2.insert(
            "source_path".into(),
            Value {
                kind: Some(Kind::StringValue(
                    "/u/.claude/projects/-u-x/x.jsonl".into(),
                )),
            },
        );
        v2.insert(
            "project_name".into(),
            Value {
                kind: Some(Kind::StringValue("x".into())),
            },
        );
        v2.insert(
            "start_iso".into(),
            Value {
                kind: Some(Kind::StringValue("2026-05-17T09:15:18.335Z".into())),
            },
        );
        v2.insert(
            "user_turns".into(),
            Value {
                kind: Some(Kind::IntegerValue(5)),
            },
        );
        v2.insert(
            "has_errors".into(),
            Value {
                kind: Some(Kind::BoolValue(true)),
            },
        );
        let p = V3Payload::from_v2_payload(&v2).unwrap();
        assert_eq!(p.session_id, "abc");
        assert_eq!(p.user_turns, 5);
        assert!(p.has_errors);
        assert_eq!(p.source_agent, "claude_code");
        assert_eq!(p.schema_version, 3);
        // start_ts_dt is copied from start_iso during the lift.
        assert_eq!(p.start_ts_dt, p.start_iso);
    }

    // -----------------------------------------------------------------
    // Issue #14 — identifier_tokens(): 6 unit tests + dual-field check
    // -----------------------------------------------------------------

    #[test]
    fn identifier_tokens_camel_case_split() {
        let t = identifier_tokens("getUserData refactor");
        // Originals + camel split parts, no length-1 noise, deduped.
        assert!(t.contains(&"getUserData".to_string()));
        assert!(t.contains(&"get".to_string()));
        assert!(t.contains(&"User".to_string()));
        assert!(t.contains(&"Data".to_string()));
        assert!(t.contains(&"refactor".to_string()));
    }

    #[test]
    fn identifier_tokens_snake_and_kebab_case_split() {
        let snake = identifier_tokens("fix_dns_resolver bug");
        assert!(snake.contains(&"fix_dns_resolver".to_string()));
        assert!(snake.contains(&"fix".to_string()));
        assert!(snake.contains(&"dns".to_string()));
        assert!(snake.contains(&"resolver".to_string()));
        assert!(snake.contains(&"bug".to_string()));

        let kebab = identifier_tokens("low-power mode");
        assert!(kebab.contains(&"low-power".to_string()));
        assert!(kebab.contains(&"low".to_string()));
        assert!(kebab.contains(&"power".to_string()));
        assert!(kebab.contains(&"mode".to_string()));
    }

    #[test]
    fn identifier_tokens_pascalcase_and_acronyms() {
        // PascalCase + acronym-with-digit: `PowerShot` and `4G`.
        let t = identifier_tokens("PowerShot 4G");
        assert!(t.contains(&"PowerShot".to_string()));
        assert!(t.contains(&"Power".to_string()));
        assert!(t.contains(&"Shot".to_string()));
        // `4G` length 2, no internal boundary → preserved as-is.
        assert!(t.contains(&"4G".to_string()));
    }

    #[test]
    fn identifier_tokens_mixed_case_and_underscores() {
        // Combined camel + snake: split on both boundaries.
        let t = identifier_tokens("getUser_fromCache");
        assert!(t.contains(&"getUser_fromCache".to_string()));
        // snake split
        assert!(t.contains(&"getUser".to_string()));
        assert!(t.contains(&"fromCache".to_string()));
        // and the camel pieces (from the snake fragments OR the original)
        assert!(t.contains(&"get".to_string()));
        assert!(t.contains(&"User".to_string()));
        // "from" appears via camel split of "fromCache"
        assert!(t.contains(&"from".to_string()) || t.contains(&"Cache".to_string()));
    }

    #[test]
    fn identifier_tokens_unicode_pass_through() {
        // Non-ASCII titles: should be preserved as a token (no crash).
        let t = identifier_tokens("디버깅 세션");
        assert!(t.contains(&"디버깅".to_string()));
        assert!(t.contains(&"세션".to_string()));
        // No internal boundaries in Hangul, so no extra splits expected.
    }

    #[test]
    fn identifier_tokens_empty_and_short_tokens_dropped() {
        // Empty input → empty vec (no panic).
        assert!(identifier_tokens("").is_empty());
        // Single-character tokens dropped to avoid index noise.
        let t = identifier_tokens("a b cd ef");
        assert!(!t.contains(&"a".to_string()), "single-char tokens dropped");
        assert!(!t.contains(&"b".to_string()), "single-char tokens dropped");
        assert!(t.contains(&"cd".to_string()));
        assert!(t.contains(&"ef".to_string()));
    }

    // -----------------------------------------------------------------
    // Issue #13 — QuantMode::from_env unit tests
    // -----------------------------------------------------------------

    /// Helper: run a closure with `MEMEX_QUANT_MODE` set to `value` (or
    /// unset if `None`), then restore the previous value. Avoids leaking
    /// env state between tests; `cargo test` runs lib tests in the same
    /// process so this matters.
    fn with_env<F, R>(value: Option<&str>, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // `cargo test` runs tests in parallel by default; serialize every
        // `with_env` call through a process-wide lock so concurrent tests can't
        // race on MEMEX_QUANT_MODE or clobber each other's save/restore
        // (Gemini review on PR #32). Recover from a poisoned lock (a panicking
        // test holding it) so one failure doesn't cascade into all the others.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = "MEMEX_QUANT_MODE";
        let saved = std::env::var(key).ok();
        // SAFETY: edition 2024 made env::set_var/remove_var `unsafe` (they are
        // unsound if another thread reads the environment concurrently). The
        // ENV_LOCK above serializes all env mutation in these tests, and the
        // helper restores the prior value around `f`.
        unsafe {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let result = f();
        unsafe {
            match saved {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        result
    }

    #[test]
    fn issue_13_quant_mode_from_env_default_is_tq_bits2() {
        with_env(None, || {
            assert_eq!(QuantMode::from_env(), QuantMode::TqBits2);
        });
    }

    #[test]
    fn issue_13_quant_mode_from_env_recognises_f32() {
        with_env(Some("f32"), || {
            assert_eq!(QuantMode::from_env(), QuantMode::F32None);
        });
        with_env(Some("none"), || {
            assert_eq!(QuantMode::from_env(), QuantMode::F32None);
        });
        // Mixed case + whitespace tolerated.
        with_env(Some("  F32  "), || {
            assert_eq!(QuantMode::from_env(), QuantMode::F32None);
        });
    }

    #[test]
    fn issue_13_quant_mode_from_env_recognises_turboquant_variants() {
        with_env(Some("tq-bits1"), || {
            assert_eq!(QuantMode::from_env(), QuantMode::TqBits1);
        });
        with_env(Some("tq-bits2"), || {
            assert_eq!(QuantMode::from_env(), QuantMode::TqBits2);
        });
        // Underscore alternative + uppercase tolerated.
        with_env(Some("TQ_BITS1"), || {
            assert_eq!(QuantMode::from_env(), QuantMode::TqBits1);
        });
    }

    #[test]
    fn issue_13_quant_mode_unknown_falls_back_to_tq_bits2() {
        // Unrecognised values must not crash — they fall back to the
        // pre-#13 production default (TqBits2) so accidentally setting
        // `MEMEX_QUANT_MODE=garbage` does not change collection shape.
        with_env(Some("garbage"), || {
            assert_eq!(QuantMode::from_env(), QuantMode::TqBits2);
        });
        with_env(Some(""), || {
            assert_eq!(QuantMode::from_env(), QuantMode::TqBits2);
        });
    }

    #[test]
    fn issue_13_quant_mode_to_inner_shapes() {
        // F32None → None (no quantization field on the builder).
        assert!(QuantMode::F32None.to_inner_quantization().is_none());
        // TqBits1 and TqBits2 each produce a Turboquant variant.
        match QuantMode::TqBits1.to_inner_quantization() {
            Some(Quantization::Turboquant(_)) => {}
            other => panic!("TqBits1 must produce Turboquant; got {other:?}"),
        }
        match QuantMode::TqBits2.to_inner_quantization() {
            Some(Quantization::Turboquant(_)) => {}
            other => panic!("TqBits2 must produce Turboquant; got {other:?}"),
        }
    }

    #[test]
    fn issue_13_quant_mode_as_name_roundtrips() {
        // Each mode's `as_name()` must round-trip back through `from_env`.
        for mode in [QuantMode::F32None, QuantMode::TqBits1, QuantMode::TqBits2] {
            let name = mode.as_name();
            with_env(Some(name), || {
                assert_eq!(
                    QuantMode::from_env(),
                    mode,
                    "round-trip failed for {name}",
                );
            });
        }
    }

    #[test]
    fn issue_14_v3_payload_carries_ai_title_tokens() {
        // V3Payload::from_session_fields must populate ai_title_tokens
        // automatically — frontend doesn't have to pass it explicitly.
        let p = V3Payload::from_session_fields(
            "sid".into(),
            "/sessions/sid.jsonl".into(),
            "memex".into(),
            "/repo/memex".into(),
            "main".into(),
            "getUserData refactor".into(),
            "1.0".into(),
            "2026-05-28T00:00:00Z".into(),
            "2026-05-28T01:00:00Z".into(),
            10,
            10,
            5,
            false,
        );
        assert!(
            p.ai_title_tokens.contains(&"getUserData".to_string()),
            "original token preserved",
        );
        assert!(
            p.ai_title_tokens.contains(&"User".to_string()),
            "camelCase split applied",
        );
        // to_qdrant_payload must round-trip the new field (it just serdes
        // self, so this is a sanity check that the field is part of the
        // serialized form).
        let pl = p.to_qdrant_payload();
        let serialized = format!("{pl:?}");
        assert!(
            serialized.contains("ai_title_tokens"),
            "ai_title_tokens must be in the qdrant payload"
        );
    }
}
