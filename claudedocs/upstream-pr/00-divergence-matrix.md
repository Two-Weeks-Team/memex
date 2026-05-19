# Upstream Divergence & Backport-Feasibility Matrix

**Generated**: 2026-05-19 (D-13)
**Fork HEAD**: `8509096` (docs commit) — fork tree-snapshot equals `e28dbee` (PR #15 merge)
**Upstream HEAD**: `4973a91` (feat: data archaeology + dashboard + Time Machine rail + watcher polish)
**Merge-base**: `a987952`
**Counted divergence**: fork +35 commits / upstream +14 commits
  (PR-plan doc says "fork +30 / upstream +13" — off-by-five because it was written before this doc plus 4 review/hotfix commits)

This document supersedes §2 of `claudedocs/UPSTREAM_PR_PLAN.md`. Where the two disagree, this matrix wins — it was built from `git diff` against the actual SHAs, not from working memory.

---

## §0 TL;DR — the realities the plan doc glossed over

1. **Schema reality is dual, not "fork-only v3"**. Fork keeps `COLLECTION = "memex_sessions"` (v2) as a read-fallback while writes target `COLLECTION_V3 = "memex_sessions_v3"`. Both collection constants live on fork. Upstream knows only v2. So a backport touching writes needs a migration note; a backport touching reads can work against v2 alone and survive on both sides.
2. **Upstream ALREADY has `tail_recent_errors`** (added in `4973a91`). Fork commit `84db1fc` improves the same function with the same signature, on `parser::parse_session` which exists upstream. This is the **only** candidate that is a near-pure cherry-pick.
3. **Upstream gained `parse_transcript_session` + `scan_transcripts_dir`** in `4973a91` (Anthropic silent-migration archaeology) — fork **does not have these**. Any fork→upstream PR that touches `parser.rs` will see a +283 line conflict on first cherry-pick attempt.
4. **`2b59dc9` (predict-Codex) cannot be backported in any form** — upstream has no Codex parser, no `source_agent` payload field, and no infrastructure to gain one without P5 KH-01 and P3 KG-03 going first. The "fix" is meaningful only inside the multi-agent payload world fork built.
5. **`deed283` (heat-trail purple oval) cannot be backported** — upstream has no `#heat-trail` SVG at all. The SVG `vector-effect="non-scaling-stroke"` pattern is potentially generalizable, but no upstream SVG site needs it today.
6. **`e402b1f` (mix-modal picker)** depends on `lens_search_v2` (fork-only command from P2) at runtime. Upstream's `mix-modal` has the same `<dialog id="mix-modal">` skeleton, so the HTML skeleton merge is small, but the JS layer needs to call `lens_search` (upstream) instead of `lens_search_v2` (fork) — that's a real rewrite, not a cherry-pick.
7. **Truly small upstream-PR-able commits are TWO**: `84db1fc` (recall filter + errors tooltip + WebView devtools) and a synthesized `mix-modal-picker-for-upstream` (manually backported from `e402b1f`).

---

## §1 File-level divergence map

`git diff --name-status upstream/main..main` over `src-tauri/src/**.rs`, `src/**`, top-level configs. Tests and fixtures are summarized; the full per-file list lives at the bottom of this section.

### 1.1 SHARED files (exist both sides, fork modified)

| File | Δ lines (fork−upstream) | Fork commits touching it | Backport surface |
|---|---:|---|---|
| `src-tauri/src/cli.rs` | **+95 / −50** | `e1c075b`, `2691006`, `5a9719d` | adds `--agent` flag, `crud::ensure_collection_v3`, Codex root scan. Top-of-file `use` line uses `codex_parser`+`crud` (fork-only). |
| `src-tauri/src/commands.rs` | **+327 / −128** | `84db1fc`, `57abfc9`, `769af65`, `7cbc588`, `2691006`, `5a9719d`, `5aa5e24`, `f55d417` | 8 new `#[tauri::command]` fns (lens_search_v2, mix_match_with_pairs, list_sessions_ordered, lens_search_grouped, relevance_feedback, snapshot_export/import, …), `sec::validate_session_path` calls, recall filter. Cherry-pick is **not viable**; 84db1fc-only slice is. |
| `src-tauri/src/indexer.rs` | **+559 / −157** | `2b59dc9`, `abdf68d`, `769af65`, `7cbc588`, `2691006`, `5a9719d`, `5aa5e24`, `f55d417` | dual-write to `COLLECTION_V3`, `build_point_v3*`, `bulk_index_legacy`, lens shim, snapshot upgraded to v3. Single-commit cherry-pick infeasible. |
| `src-tauri/src/lib.rs` | **+30 / −39** | `70121fb`, `abdf68d`, `7cbc588`, `2691006`, `5a9719d`, `5aa5e24`, `f55d417` | declares 12 new fork-only modules (`codex_parser`, `crud`, `embed_late`, `embed_pool`, `enrich`, `eval_ndcg`, `insights_cache`, `lens`, `parse_cache`, `payload`, `retrieval`, `schema`, `sec`, `snapshot`) and deletes `mcp`+`watcher`. Cannot be cherry-picked at all. |
| `src-tauri/src/main.rs` | **+0 / −15** | (only deletion via merges) | upstream has CLI bootstrap fork removed when CLI was moved into `cli.rs`. Trivial. |
| `src-tauri/src/parser.rs` | **+0 / −283** | (no fork commits — divergence is upstream-only addition) | **upstream added 283 lines fork lacks** (`parse_transcript_session`, `scan_transcripts_dir`). Any fork-side parser change will conflict. |
| `src-tauri/Cargo.toml` | **+18 / −2** | `84db1fc`, `70121fb`, `769af65`, `2691006`, `f55d417` | adds `tauri-plugin-deep-link`, `argon2`, `ed25519-dalek`, `quick-xml`, `xz2`, `ndarray`, `bm25`, `rayon`, devtools feature. 84db1fc's one-line devtools change is the only trivially-portable piece. |
| `src-tauri/capabilities/default.json` | **+2 / −1** | `70121fb` | adds `deep-link:default`. P8-only. |
| `src-tauri/tauri.conf.json` | **+8 / 0** | `70121fb` | registers `memex://` deep-link scheme. P8-only. |
| `src/index.html` | **+172 / −15** | `e402b1f`, `138da86` | WOW surfaces (heat-trail SVG, heat-chip, hyperplane canvas, agent-filter, gap-overlay, mix-picker). 138da86 alone is +137 lines of WOW chrome; e402b1f adds the picker. |
| `src/main.js` | **+1673 / −372** | `deed283`, `84db1fc`, `712a128`, `e402b1f`, `b4b205a`, `70121fb`, `769af65`, `138da86` | Grew 1851 → 3152 lines (+70%). All WOW behavior + deep-link router + view-transition cleanup + mix-picker logic. |
| `src/styles.css` | **+807 / −192** | `deed283`, `0f29b96`, `e402b1f`, `138da86` | Heat-trail / heat-chip / hyperplane / agent-filter / gap-overlay / mix-picker CSS, plus reduced-motion fallbacks. |
| `.gitignore` | **+11 / 0** | (P8 evidence dirs) | trivial |
| `README.md` | **+0 / −107** | (replaced with hackathon-flavored copy) | not portable |

### 1.2 FORK_ONLY files (added on fork, absent upstream)

**Rust source — 14 modules, ~6,300 LOC**:

| Module | LOC | Phase | Direct dependents (in fork) |
|---|---:|---|---|
| `codex_parser.rs` | 777 | P5 KH-01 | `commands.rs`, `indexer.rs`, `cli.rs` |
| `lens.rs` | 1336 | P2 + P4 | `commands.rs`, `indexer.rs` |
| `enrich.rs` | 871 | P5 | `indexer.rs` |
| `schema.rs` | 854 | P3 | `cli.rs`, `crud.rs`, `indexer.rs`, `lens.rs`, `retrieval.rs` |
| `retrieval.rs` | 798 | P4 | `commands.rs`, `indexer.rs` |
| `crud.rs` | 564 | P5 | `commands.rs`, `indexer.rs`, `cli.rs` |
| `snapshot.rs` | 506 | P1 KF-02/03 | `commands.rs` |
| `insights_cache.rs` | 311 | P5 | `indexer.rs` |
| `sec.rs` | 284 | P1 KF-01 | `commands.rs`, `indexer.rs`, `schema.rs` |
| `payload.rs` | 212 | review-pass | `indexer.rs`, `lens.rs`, `retrieval.rs` |
| `embed_late.rs` | 175 | P4 | `indexer.rs` |
| `parse_cache.rs` | 174 | P5 KG-02 | `indexer.rs` |
| `eval_ndcg.rs` | 160 | P5 | (none — standalone bin support) |
| `embed_pool.rs` | 150 | P5 KG-01 | `indexer.rs` |

**Tests** (10 integration files, ~2,300 LOC): `codex_parser_integration`, `lens_integration`, `retrieval_integration`, `schema_integration`, `sec_integration`, `snapshot_integration`, plus `tests/parser.rs` modified. Fixtures: 5 Codex rollout JSONLs, 3 schema fixtures.

**Scripts / docs** (not relevant to source-level backport): `scripts/demo/*.sh`, `claudedocs/IMPLEMENTATION_REPORT.md`, `claudedocs/phases/phase-7-demo-production/video-script.md`, `tests/e2e/README.md`.

### 1.3 UPSTREAM_ONLY files (deleted on fork, present upstream)

| File | Upstream blob | Why fork deleted | Backport impact |
|---|---|---|---|
| `src-tauri/src/mcp.rs` | 482 lines | Hackathon scope shed MCP server (P0 decision) | Any upstream PR is fine; fork must accept it stays absent on fork side. |
| `src-tauri/src/watcher.rs` | 578 lines | Hackathon scope shed background watcher (P0) | Same. |
| `src/dashboard.html` | 515 lines | Dashboard replaced by main view | Same. |
| `src/dashboard.js` | 836 lines | Same | Same. |
| `docs/IMPL-MCP.md` | 89 lines | Doc for MCP | Same. |
| `src-tauri/tests/fixtures_history/history.jsonl` | 8 lines | Watcher test fixture | Same. |
| `src-tauri/tests/fixtures_transcripts/ses_*.jsonl` | 7 lines (2 files) | Transcripts/legacy parser was upstream-only | Same. |

**Conclusion**: fork→upstream PRs do not need to touch any of these — they are the upstream maintainer's territory. Fork's deletion of them is the fork's own product decision, not a delta the upstream cares about.

---

## §2 Module dependency graph for fork-only modules

```
                       ┌──────────────┐
                       │  schema.rs   │  ──► COLLECTION_V3, V3Payload, infer_source_agent
                       │  854 LOC     │      (KH-01 source_agent FieldType::Keyword)
                       └──────┬───────┘
            ┌─────────────────┼─────────────────────┐
            ▼                 ▼                     ▼
        ┌────────┐       ┌──────────┐         ┌────────────┐
        │ cli.rs │       │ crud.rs  │         │ payload.rs │ ── payload_str / payload_i64 / payload_to_json
        │ (mod)  │       │  564 LOC │         │  212 LOC   │   (extracted out of indexer/retrieval/lens to
        └────────┘       └─────┬────┘         └──────┬─────┘    avoid duplication, per Gemini review)
                               │                     │
                               ▼                     ▼
                       ┌──────────────┐       ┌─────────────┐
                       │  indexer.rs  │ ◄─── │ retrieval.rs │ ── lens_search_grouped, list_sessions_ordered,
                       │  +559 / −157 │       │   798 LOC   │     relevance_feedback, mix_match_with_pairs
                       │  vs upstream │       └─────────────┘
                       └─┬─────┬────┬┘
                ┌────────┘     │    └───────────┐
                ▼              ▼                ▼
         ┌─────────────┐  ┌──────────┐    ┌──────────────┐
         │ codex_parser│  │ lens.rs  │    │  enrich.rs   │ ── derive_outcome, has_errors,
         │   777 LOC   │  │ 1336 LOC │    │   871 LOC    │    intent/arc/topic enrichment
         └──────┬──────┘  └─────┬────┘    └──────┬───────┘
                │               │                │
                ▼               ▼                ▼
         ┌──────────────┐  ┌──────────────┐  ┌────────────────┐
         │   sec.rs     │  │ embed_late   │  │ insights_cache │
         │   284 LOC    │  │   175 LOC    │  │    311 LOC     │
         │ (SourceAgent,│  │ (multivec    │  │ (LRU on enrich │
         │  sandbox)    │  │  embedder)   │  │  results)      │
         └──────┬───────┘  └──────────────┘  └────────────────┘
                │
                ▼
         ┌──────────────┐
         │ snapshot.rs  │  ── signed envelope (ed25519), SnapshotSandbox, CURRENT_SCHEMA_VERSION=3
         │   506 LOC    │     (no other dependents besides commands.rs)
         └──────────────┘

         (also: embed_pool 150, parse_cache 174, eval_ndcg 160 — each only feeds indexer.rs)
```

### What this means for backport closures

If you cherry-pick **only** one fork-only module, the others it `use`s must come along.
The minimum-viable backport bundles are:

| Goal | Minimum modules to drag along | Total LOC (modules + tests) |
|---|---|---:|
| `codex_parser` alone | `codex_parser`, `sec` (for SourceAgent enum) | ~1,000 |
| Real Codex E2E (parse + index + search) | `codex_parser`, `sec`, `schema`, `crud`, `payload`, + indexer.rs/commands.rs/cli.rs/lib.rs rewrites | ~3,500 |
| P1 sec sandbox alone | `sec` + (mods to `commands.rs`+`indexer.rs` to wire `validate_session_path`) | ~400 |
| P1 snapshot signed envelope | `snapshot` + Cargo deps (`sha2`, `ed25519-dalek`) | ~600 |
| Lens v2 query API | `lens`, `payload`, `schema`, `retrieval`, + indexer.rs rewrite (Embedder, FormulaQuery) | ~3,500 |
| Everything (P1+P3+P4+P5) | All 14 modules + indexer/commands/lib/cli rewrites | ~8,000 |

There is **no** fork-only module whose dependency closure stays under ~400 LOC except `sec` (and only if you accept a partial backport that adds the sandbox primitive without enabling Codex).

---

## §3 Per-commit backport-feasibility verdict

Format: ✅ feasible / ⚠️ requires rewrite / ❌ infeasible.

### 3.1 `e402b1f` — fix(mix): self-contained Mix & Match picker

| Aspect | Value |
|---|---|
| Files touched | `src/index.html` (+35/−3), `src/main.js` (+148/−10), `src/styles.css` (+127/0) |
| Pure cherry-pick | ❌ 3-way conflicts on all 3 files |
| Manual rewrite feasible | ✅ yes — upstream has `<dialog id="mix-modal">` with the same dropzone skeleton, same `addToMix()` JS entry point |
| Hard runtime dependency | **`lens_search_v2`** (fork-only Tauri command, lives in `lens.rs` + `commands.rs`). Picker search calls `invoke("lens_search_v2", …)`. Upstream has only `lens_search`. |
| Conflict driver in main.js | 1851 → 3152 lines context divergence; mix-modal-specific changes are localized but surrounding code is heavily rewritten |
| Schema impact | none — picker uses session_id strings only |
| **Verdict** | **⚠️ Backport feasible IF the picker is rewired to call `lens_search` (upstream) instead of `lens_search_v2`. Estimated effort: 1–2 hours. PR-able as a standalone "Mix & Match modal usability fix" against upstream.** |

### 3.2 `f55d417` — P1 security (KF-01 sandbox + KF-02 snapshot + KF-03 signed envelope + KH-01 multi-agent)

| Aspect | Value |
|---|---|
| Files touched | `Cargo.lock/toml` (+2/+2), `commands.rs` (+43/−), `indexer.rs` (+8/−), `lib.rs` (+2/−), `sec.rs` (+279, new), `snapshot.rs` (+439, new), `tests/sec_integration.rs` (+30, new), `tests/snapshot_integration.rs` (+30, new) |
| Pure cherry-pick | ❌ — `sec.rs` references `SourceAgent::Codex` which assumes Codex paths exist; `snapshot.rs` references `CURRENT_SCHEMA_VERSION = 3` (fork-only schema constant) |
| Manual split possible | ⚠️ partial — KF-01 path sandbox could be sliced off **IF** `SourceAgent` is reduced to `{ClaudeCode}` (no Codex), `validate_session_path` validates against the single Claude root, and snapshot bits are dropped. That subset is ~120 LOC. |
| KF-03 signed envelope blockers | hard-codes `CURRENT_SCHEMA_VERSION: u32 = 3`; backporting to upstream (v2 only) needs that demoted to 2, which then breaks fork |
| **Verdict** | **⚠️ KF-01 path sandbox subset is feasible as a standalone "harden session-path validation" PR (~120 LOC, single file, 1 integration test). KF-02/03/KH-01 are infeasible without P3+P5 going first.** |

### 3.3 `2b59dc9` — fix(predict): route Codex sessions through codex_parser

| Aspect | Value |
|---|---|
| Files touched | `src-tauri/src/indexer.rs` (+39/−5) |
| Pure cherry-pick | ❌ |
| Why it's infeasible upstream | (a) Upstream has no `crate::codex_parser` module. (b) Upstream payload has no `source_agent` field (KH-01 is P5, schema.rs is fork-only). (c) Upstream `predict_next_actions` lives at line 1244 (not 1604), API surface is identical but the function body unconditionally calls `parser::parse_session` — the bug being "fixed" doesn't exist upstream because upstream has no Codex sessions to mis-parse in the first place. |
| Possible rewrite as upstream PR | ❌ none — the fix is a no-op without a multi-agent payload world |
| **Verdict** | **❌ INFEASIBLE. Skip. This fix only makes sense inside the fork's P3+P5 universe.** |

### 3.4 `deed283` — fix(ux): kill viewport-spanning purple oval (heat-trail stroke explosion)

| Aspect | Value |
|---|---|
| Files touched | `src/main.js` (+59/−5), `src/styles.css` (+8/−1) |
| Pure cherry-pick | ❌ |
| Why infeasible | Upstream has no `#heat-trail` SVG, no `drawHeatTrail()`, no `HEAT_COLOR_*` constants, no `.heat-trail`/`.heat-chip` CSS. The bug being fixed doesn't exist upstream. |
| Generalizable pattern | The `vector-effect="non-scaling-stroke"` + viewBox clamp pattern is a real SVG anti-pattern hardening that **could** be applied to upstream if upstream introduces user-zoomable SVG. As of `4973a91`, it has not. |
| **Verdict** | **❌ INFEASIBLE. Nothing to fix.** |

### 3.5 `e1c075b` — fix(cli): ensure v3 collection before bulk index

| Aspect | Value |
|---|---|
| Files touched | `src-tauri/src/cli.rs` (+8/−2) |
| Pure cherry-pick | ❌ — the fix calls `crud::ensure_collection_v3` and reads `crate::schema::COLLECTION_V3`, neither of which exists upstream |
| Inverted port (upstream's perspective) | The bug doesn't exist upstream because upstream has no v3 collection. `ensure_collection` (v2) already runs on upstream's `cmd_scan`. |
| **Verdict** | **❌ INFEASIBLE. The bug is fork-private — v2 ensure already covers upstream's single-collection world.** |

### 3.6 `0f29b96` — fix(ux): heat-chip "giant purple oval" — clamp width, single-line, drop backdrop-filter

| Aspect | Value |
|---|---|
| Files touched | `src/styles.css` (+34/−4) |
| Pure cherry-pick | ❌ — selectors `.heat-chip*` don't exist upstream |
| **Verdict** | **❌ INFEASIBLE. Same root cause as 3.4 — no upstream surface to apply the CSS to.** |

### 3.7 `712a128` — fix(ux): release stuck view-transition snapshot + heat-chip top-center + recall banner auto-hide

| Aspect | Value |
|---|---|
| Files touched | `src/main.js` (+72/−4) |
| Three concerns | (a) View Transitions cleanup for `cinematicZoom`. (b) Heat-chip top-center positioning. (c) Recall banner `autoHideTimer`. |
| (a) cinematicZoom backport | ❌ infeasible — `cinematicZoom` is WOW-4 fork-only (predict thumbnail → Replay morph). Upstream has no such transition. |
| (b) heat-chip positioning | ❌ infeasible — no upstream heat-chip. |
| (c) recall banner auto-hide | ⚠️ **could** be backported in isolation — upstream has a recall banner (since `4973a91` added `tail_recent_errors` polling). The auto-hide pattern is ~15 LOC of vanilla DOM + setTimeout. But it presupposes upstream's recall banner UX, which renders differently (no `state.banner` object structure — manual check needed). |
| **Verdict** | **⚠️ Partial backport (recall banner auto-hide ONLY, ~15 LOC) is conceivable but adds little value — upstream maintainer would probably accept a much smaller targeted PR rather than a slice of this one. Not recommended.** |

### 3.8 `84db1fc` — fix(ux+observability): enable WebView devtools + silence shell-stderr recall + add errors-badge tooltip

| Aspect | Value |
|---|---|
| Files touched | `src-tauri/Cargo.toml` (+1/−1), `src-tauri/src/commands.rs` (+32/−7), `src/main.js` (+13/−1) |
| Cargo.toml change | one-line `features = ["tray-icon", "devtools"]` — upstream Cargo.toml has the same feature list as fork's pre-fix state. **Trivial cherry-pick.** |
| commands.rs change | improves `tail_recent_errors` — function exists upstream verbatim (same signature, same return type, same `parser::parse_session` dependency, same `default_projects_root` helper). The +32/−7 diff is a localized rewrite of the body-text regex into structured-only + shell-noise blocklist. **Highly portable.** |
| main.js change | adds `title=` attribute to the `errors` badge — upstream has the exact same `'<span class="badge err">errors</span>'` markup at line 454. **Trivial cherry-pick of just the literal.** |
| Schema impact | none |
| Conflict risk | low — only the `commands.rs` body of `tail_recent_errors` is touched, surrounding code (cache machinery, RecentError struct) is identical |
| **Verdict** | **✅ FEASIBLE as a near-direct cherry-pick. Three small, independently meaningful improvements bundled in one commit — the cleanest upstream PR candidate in the entire fork.** |

### 3.9 Other commits (not in the candidate list but worth noting)

| Commit | Status |
|---|---|
| `5aa5e24` (P3 schema v3) | ❌ Infeasible alone — the entire fork rests on it. Would be a "feature drop" of P3 if attempted. |
| `5a9719d` (P4 retrieval) | ❌ depends on P3 |
| `2691006` (P5 perf+enrich+codex) | ❌ depends on P3+P4 |
| `7cbc588` (P2 query API) | ❌ depends on P3 |
| `138da86` (P6 WOW surfaces) | ❌ adds 5 frontend surfaces; cosmetic-only port would require upstream agreement on each surface |
| `540281a` (P7 demo scripts) | ⚠️ feasibility N/A — it's `scripts/demo/*.sh` + a video script, no code change. Not really a PR candidate. |
| `70121fb` (P8 E2E + deep-links) | ❌ adds `tauri-plugin-deep-link`, capability + conf changes, deep-link router in main.js. Cosmetic+infra mix. |
| `b4b205a`, `113665f`, `abdf68d`, `769af65`, `57abfc9` (review polish) | ❌ chained on P-series — no standalone meaning |

---

## §4 Schema compatibility check (the dual-write reality)

### 4.1 Collection constants

| Constant | Upstream | Fork |
|---|---|---|
| `COLLECTION` (`indexer.rs:37`) | `"memex_sessions"` (writes here) | `"memex_sessions"` (read-fallback ONLY) |
| `COLLECTION_V3` (`schema.rs:34`) | (does not exist) | `"memex_sessions_v3"` (write target) |
| Write path | `UpsertPointsBuilder::new(COLLECTION, …)` | `UpsertPointsBuilder::new(crate::schema::COLLECTION_V3, …)` (see indexer.rs:478, :812) |
| Read path | `COLLECTION` only | `COLLECTION_V3` first, falls back to `COLLECTION` for KG-03 dual-read (indexer.rs:1126 comment "v3 first, v2 fallback so topology still has labels") |
| Snapshot target | `format!("{url}/collections/{COLLECTION}/snapshots")` | `format!("{url}/collections/{collection}/snapshots")` where `collection = crate::schema::COLLECTION_V3` (indexer.rs:1928) |

### 4.2 Payload shape divergence

Upstream payload (v2, defined inline in indexer.rs's session-to-Payload helper):
- `session_id`, `source_path`, `project_path`, `project_name`, `git_branch`, `claude_version`, `ai_title`, `start_ts`, `end_ts`, `turn_count`, `user_count`, `assistant_count`, `total_tool_calls`, `total_tool_results`, `has_errors`, `error_signature_*` …

Fork v3 payload (defined in `schema.rs::V3Payload`):
- All of the above MINUS numeric `start_ts`/`end_ts` (replaced with `start_iso`/`end_iso` datetime-indexed strings — see indexer.rs:386 comment).
- PLUS: `source_agent` (KH-01, FieldType::Keyword, schema.rs:252), `intent`, `entities`, `outcome`, `arc`, `topic` (P5 KE-01 enrichment fields).

**Conclusion**: any backport that writes payload upstream MUST use the v2 shape (numeric `start_ts`/`end_ts`, no `source_agent`, no enrichment fields). Conversely, any upstream backport that reads `source_agent` will return None on every upstream-indexed point. The two shapes are **NOT** wire-compatible for round-trips.

### 4.3 Schema migration note for any payload-touching upstream PR

If an upstream PR introduces a new payload field, the fork backport must either:
1. Add it to `V3Payload` in `schema.rs` (and update `to_qdrant_payload`), OR
2. Add it to the v2-write helper in `indexer.rs` (which fork still keeps as `bulk_index_legacy` but no longer calls).

Conversely, if a fork→upstream PR touches payload, the PR description **MUST** note: "upstream collection name is `memex_sessions` (v2 shape); this PR adds field X only to the v2 shape — fork carries the v3 mirror separately."

The three candidates from §3 (84db1fc, sliced-mix-modal, KF-01 sandbox-only) **do not touch payload**, so no migration note is needed for them.

---

## §5 Recommended PR slicing

### 5.1 PR #1 — RECOMMENDED — upstream's `tail_recent_errors` filter cleanup + WebView devtools + errors-badge tooltip

**Source**: cherry-pick `84db1fc` essentially as-is.

**Files**: `src-tauri/Cargo.toml`, `src-tauri/src/commands.rs`, `src/main.js`.

**Conflict risk**: low — surrounding code identical, only intra-function diffs.

**Procedure**:
```bash
git checkout -b backport/recall-filter-and-devtools upstream/main
git cherry-pick 84db1fc
# expect: trivial 3-file context fixup, no semantic conflicts
cargo test --lib
git push origin HEAD
gh pr create --repo sgwannabe/memex --base main \
  --title "fix(ux+observability): silence shell-stderr recall + WebView devtools + errors-badge tooltip" \
  --body  "(adapt 84db1fc's commit message body)"
```

**Pitch to maintainer**: three independent, small improvements bundled because they share the same "let the user understand what they're seeing" theme. Each is reviewable on its own.

**Size estimate**: <80 net LOC.

### 5.2 PR #2 — RECOMMENDED — Mix & Match self-contained picker (manual rewrite)

**Source**: `e402b1f` rewritten against upstream's mix-modal HTML skeleton + `lens_search` (not `lens_search_v2`).

**Files**: same 3 (`index.html`, `main.js`, `styles.css`).

**Conflict risk**: high if cherry-picked, low if manually rewritten.

**Procedure**:
```bash
git checkout -b backport/mix-modal-self-contained-picker upstream/main
# manually port:
#  - src/index.html: insert `.mix-picker` block inside <dialog id="mix-modal">
#  - src/main.js:   add runMixPickerSearch/renderMixPickerRow/attachMixPickerEvents
#                   that call invoke("lens_search", …) — NOT lens_search_v2
#  - src/styles.css: add .mix-picker* selectors + .btn[disabled]
cargo test --lib   # no Rust change, but run anyway
npm run tauri build && open … && manually verify modal
git push origin HEAD
gh pr create --repo sgwannabe/memex --base main \
  --title "fix(mix): self-contained Mix & Match picker — modal no longer depends on cards behind backdrop" \
  --body  "(adapt e402b1f's commit message body, noting the lens_search-not-v2 substitution)"
```

**Pitch to maintainer**: real UX bug (showModal()'s backdrop blocks the buttons the modal asks the user to press); fix is self-contained in the modal subtree.

**Size estimate**: ~315 LOC additions (matches e402b1f), but with the `lens_search_v2` → `lens_search` rewrite.

### 5.3 PR #3 — STRETCH — KF-01 path sandbox primitive only

**Source**: extract `sec.rs::validate_session_path` (single-agent variant) + wire it into `commands.rs::get_session_turns` and similar.

**Procedure**: rewrite, not cherry-pick. ~120 LOC across `src-tauri/src/sec.rs` (new, simplified) + `src-tauri/src/commands.rs` (3–4 call-site additions) + 1 integration test.

**Pitch to maintainer**: security hardening — tampered Qdrant payload could otherwise read arbitrary files via the IPC fs-read commands. Generalizes cleanly to upstream's single ~/.claude/projects/ root.

**Risk**: maintainer may want to fold this into their own architecture rather than accept a `sec.rs` module. Worth a brief "interest?" issue before opening a PR.

### 5.4 NOT recommended as upstream PRs

- `2b59dc9` (predict-Codex): infeasible.
- `deed283`, `0f29b96`, `712a128`: heat-trail / heat-chip fixes — no upstream surface.
- `e1c075b`: bug doesn't exist upstream.
- P3/P4/P5/P6/P7/P8 feature commits: too large, too many dependencies, and the maintainer added their own Codex-orthogonal direction (`4973a91` data archaeology + dashboard + macOS Time Machine rail).

### 5.5 Suggested sequence

1. **Open PR #1 first** (smallest, lowest risk). Use it to gauge maintainer review style and acceptance criteria.
2. **If PR #1 lands**, open PR #2 (mix-modal).
3. **If PR #1+#2 both land**, file a brief issue asking about appetite for `sec.rs` path sandbox before doing PR #3.
4. **Hold all P-series feature drops** until D-0 (2026-06-01) hackathon submission is done. Then consider opening a single "long-form RFC" issue inviting the maintainer to discuss whether they want any of fork's P3+P5 direction folded in.

---

## §6 Appendix — divergent file inventory (full)

`git diff --name-status upstream/main..main`:

```
M  .gitignore                                                   (P8 evidence dirs)
M  README.md                                                    (hackathon copy)
A  claudedocs/IMPLEMENTATION_REPORT.md                          (fork-only docs)
A  claudedocs/UPSTREAM_PR_PLAN.md                               (fork-only docs)
A  claudedocs/phases/phase-7-demo-production/video-script.md    (fork-only docs)
D  docs/IMPL-MCP.md                                             (upstream MCP plan, fork deleted)
M  index.html                                                   (root, fork has WOW chrome)
A  scripts/demo/capture-screenshots.sh                          (P7)
A  scripts/demo/record-demo.sh                                  (P7)
A  scripts/demo/smoke-test.sh                                   (P7)
M  src-tauri/Cargo.lock                                         (deps added)
M  src-tauri/Cargo.toml                                         (deps added)
M  src-tauri/capabilities/default.json                          (deep-link)
M  src-tauri/src/cli.rs                                         (multi-agent CLI)
A  src-tauri/src/codex_parser.rs                                (P5 KH-01)
M  src-tauri/src/commands.rs                                    (8 new commands + sec wiring)
A  src-tauri/src/crud.rs                                        (P5)
A  src-tauri/src/embed_late.rs                                  (P4)
A  src-tauri/src/embed_pool.rs                                  (P5 KG-01)
A  src-tauri/src/enrich.rs                                      (P5)
A  src-tauri/src/eval_ndcg.rs                                   (P5)
M  src-tauri/src/indexer.rs                                     (v3 write path)
A  src-tauri/src/insights_cache.rs                              (P5)
A  src-tauri/src/lens.rs                                        (P2+P4)
M  src-tauri/src/lib.rs                                         (12 new mods, 2 deleted)
M  src-tauri/src/main.rs                                        (15 lines deleted)
D  src-tauri/src/mcp.rs                                         (fork deleted MCP)
A  src-tauri/src/parse_cache.rs                                 (P5 KG-02)
M  src-tauri/src/parser.rs                                      (no-op vs upstream; upstream +283 fork lacks)
A  src-tauri/src/payload.rs                                     (review-pass refactor)
A  src-tauri/src/retrieval.rs                                   (P4)
A  src-tauri/src/schema.rs                                      (P3 v3)
A  src-tauri/src/sec.rs                                         (P1 KF-01)
A  src-tauri/src/snapshot.rs                                    (P1 KF-02/03)
D  src-tauri/src/watcher.rs                                     (fork deleted watcher)
M  src-tauri/tauri.conf.json                                    (deep-link scheme)
A  src-tauri/tests/codex_parser_integration.rs                  (P5)
A  src-tauri/tests/fixtures/schema/migration-expected.json      (P3)
A  src-tauri/tests/fixtures/schema/v2-sample.json               (P3)
A  src-tauri/tests/fixtures/schema/v3-sample.json               (P3)
A  src-tauri/tests/fixtures_codex/README.md                     (P5)
A  src-tauri/tests/fixtures_codex/rollout-01..05*.jsonl         (P5, 5 files)
D  src-tauri/tests/fixtures_history/history.jsonl               (was watcher fixture)
D  src-tauri/tests/fixtures_transcripts/ses_*.jsonl             (was legacy parser, 2 files)
A  src-tauri/tests/lens_integration.rs                          (P2+P4)
M  src-tauri/tests/parser.rs                                    (118 line diff)
A  src-tauri/tests/retrieval_integration.rs                     (P4)
A  src-tauri/tests/schema_integration.rs                        (P3)
A  src-tauri/tests/sec_integration.rs                           (P1)
A  src-tauri/tests/snapshot_integration.rs                      (P1)
D  src/dashboard.html                                           (fork deleted dashboard)
D  src/dashboard.js                                             (fork deleted dashboard)
M  src/index.html                                               (WOW surfaces + mix-picker)
M  src/main.js                                                  (1851 → 3152 lines, +70%)
M  src/styles.css                                               (heat-trail/heat-chip/hyperplane CSS)
A  tests/e2e/README.md                                          (P8)
```

Total: 14 modified, 33 added, 8 deleted = 55 file divergences. Net source LOC: +14,943 / −4,144.
