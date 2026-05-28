# Memex × Qdrant — improvement GOAL spec (audit-first, phased)

**Status**: ready for `/goal` pickup in the next session
**Generated**: 2026-05-28 (this session)
**Workflow**: **AUDIT → PLAN → IMPLEMENT → VERIFY → DEPLOY** (5 phases)
**Source decisions**: user confirmed via 4 AskUserQuestion answers in this session

**Companion docs** (all locked-in baseline):
- `claudedocs/qdrant-feature-comparison.md` — initial code-vs-Qdrant feature matrix
- `claudedocs/qdrant-improvement-plan.md` — 5-tier proposal (36 items)
- `claudedocs/qdrant-audit-findings.md` — **1st-pass AUDIT — the FACT baseline this goal builds on**
- THIS doc — the audit-first phased `/goal` spec

---

## §0 · User decisions (locked, 2026-05-28)

| # | Question | Locked answer |
|---|---|---|
| 1 | Overall scope | **Full scope = Tier 1 + 2 + 3 + 5** (accuracy + landing surfacing + engine activation + docs sync) |
| 2 | Engine priority (within Tier 3) | **All items simultaneously** (3.A–3.F together) |
| 3 | New interactive demos | **Both** — RelevanceFeedback playground (T2.13) **and** Hybrid lane visualizer (T2.14) |
| 4 | Docs priority (within Tier 5) | **All items in detail** (5.1–5.5) |
| **5** | **Landing vs engine release path** | **Landing (Tier 1 + 2 + 5 docs): direct gh-pages deploy** as before. **Engine (Tier 3 — any change in `src-tauri/`): stop at OPEN PR. DO NOT MERGE.** Team review required before merge. |
| **6** | **Docker server variant is a 1st-class target** | Every engine change (Tier 3) must work in BOTH the desktop (Tauri) AND the all-in-one Docker server variant (`deploy/web/` → image `memex-allinone` exposing `:8765` for web UI · JSON API · HTTP MCP). New `/metrics` endpoint ships ON the Docker variant. `cargo check --features web` and a docker build smoke test are mandatory in Phase 3. |

User ground rules (verbatim):
> "시간과 범위에 제한을 두지 말고 제대로 구현과 랜딩 모두 개선해야합니다."
> "이 결정사항을 포함하여 문서화를 모두 진행하세요."
> "다음세션에서 goal로 진행하겠습니다."
> "goal에서도 가정을 모두 AUDIT하고 나서 진행할 수 있도록 정리하세요."
> "다음 세션에서도 AUDIT 우선 → 계획 → 구현 → 검증 → 완전한 랜딩 구현까지 진행될 수 있도록 하세요."
> **"랜딩은 직접 디플로이까지 진행하되, 기능구현은 PR까지만 진행. (팀원과 회의후 병합해야함)"**

---

## §1 · The five phases

Each phase has a clear gate. The goal cap (200 turns) is **not** a per-phase budget — it's the absolute ceiling.

### Phase 0 · **AUDIT** (re-verify the baseline)

**Purpose**: confirm `claudedocs/qdrant-audit-findings.md` §9 checklist (A-J) still holds. If any item has drifted (e.g., another PR merged that activated `content_late`, or a teammate added `/metrics`), reconcile by updating `qdrant-audit-findings.md` BEFORE Phase 1.

**Gate (passes when)**:
- All 10 audit checklist items A-J in `qdrant-audit-findings.md` §9 produce expected results
- Any drift documented in a new "§12 · drift since 2026-05-28" section of the audit doc

**Commands to run** (surface output in conversation):
```bash
# A. SDK version
grep -E '"qdrant-client".*version' src-tauri/Cargo.lock | head -3
# B. server pin
grep -E 'image:\s*qdrant' deploy/web/docker-compose.yml
# C. content_late default
grep -A 12 'impl Default for LensWeights' src-tauri/src/lens.rs
# D. sparse activation gate
grep -nA 5 'fn active_sparse_specs' src-tauri/src/lens.rs
# E. wrapped scroll vs facets
grep -nE 'facet|Facet|scroll' src-tauri/src/wrapped.rs | head -10
# F. /metrics presence
grep -nE '/metrics|/api/health' src-tauri/src/web.rs
# G. frontend feedback
grep -nE 'relevance_feedback|👍|👎' src/main.js | head
# H. arch SVG wording
grep -E 'BINARY-QUANTIZED|TurboQuant' index.html
# I. Q1 card wording
sed -n '/data-q="Q1"/,/<\/article>/p' index.html | grep -E 'binary|TurboQuant|BQ'
# J. SDK Facet builder
find ~/.cargo/registry/src -path '*qdrant-client-1.18*' -name 'test_facets.rs'
```

**Output of Phase 0**: a short audit-confirmation note in the conversation; updated `qdrant-audit-findings.md` if any A-J drifted.

---

### Phase 1 · **PLAN reconciliation**

**Purpose**: with the audit FACT baseline confirmed (Phase 0), reconcile this goal's task list (§2) against current code. Adjust tasks that became unnecessary (e.g., if `content_late` already activated). Produce a final, ordered task list of N atomic tasks for Phase 2.

**Gate (passes when)**:
- A `TodoWrite` task list exists with all 27 atomic tasks below (or audit-adjusted equivalent)
- Each task has a confirmed file path AND an acceptance check that's grep'able

**Output of Phase 1**: the locked task list in the conversation, kept up to date with TodoWrite as Phase 2 progresses.

---

### Phase 2 · **IMPLEMENT**

**Purpose**: execute every task in the locked list. Suggested order (dependency-safe):

1. **Tier 1** (4 tasks) — accuracy fixes on landing + docs (fastest, lowest risk)
2. **Tier 2.B** (9 tasks) — small-things bullets (cheap landing-only change)
3. **Tier 5 early** (T5.1, T5.2, T5.5) — docs writing (no code execution risk)
4. **Tier 2.A** (3 tasks) — new Q7 + Q8 cards (HTML + CSS + entry animations)
5. **Tier 2.C** (2 tasks) — RelevanceFeedback playground + Hybrid lane visualizer (JS-heavy)
6. **Tier 3.3** — default-weight tuning (single-file Rust edit + eval test)
7. **Tier 3.1** — Facets in `wrapped.rs` (new code + new test)
8. **Tier 3.2** — `/metrics` endpoint (isolated new code)
9. **Tier 3.4** — custom analyzer research (apply if SDK supports, else doc)
10. **Tier 5 late** (T5.3, T5.4) — benchmarks + sequence diagrams (after engine work so numbers are real)

Each task's "definition of done" = its acceptance check in §2 passes.

**Gate (passes when)**: every task marked completed in TodoWrite AND its acceptance check verified.

---

### Phase 3 · **VERIFY**

**Purpose**: run the full validation suite (validator + node + html.parser + cargo + tests) against the final state. Surface ALL evidence in conversation.

**Gate (passes when)**: every CHECK line in §3 surfaces successfully.

---

### Phase 4 · **RELEASE** (split — landing deploys, engine stops at PR)

Per user decision #5, the landing and engine release paths are different. Phase 4 has three sub-gates: 4A, 4B, 4C.

#### Phase 4A · Landing direct deploy (Tier 1 + 2 + 5 docs)
**Purpose**: push landing changes (`index.html`, `docs/qdrant-features.md`, `docs/wired-but-dormant.md`, `docs/benchmarks.md`, `docs/architecture.md`) to gh-pages.

**Gate (passes when)**:
- gh-pages push successful with new commit hash
- `curl -s https://two-weeks-team.github.io/memex/` returns 200 AND grep confirms all new content (TurboQuant ≥1, Q7/Q8 cards present, feedback playground + hybrid viz present)
- `/forge` gallery still 200 (preservation)

#### Phase 4B · Engine PR — STOP at OPEN PR (do NOT merge)
**Purpose**: push engine changes (`src-tauri/src/{lens,wrapped,web,schema}.rs` and friends, `Cargo.toml`, any new tests) on a feature branch and open a PR for team review.

**Gate (passes when)**:
- Source branch pushed to `feat/qdrant-uplift` (or chosen base)
- `gh pr create --draft` (or `--ready`) opened against `main` with a body referencing this goal doc + the audit findings doc
- PR body includes: summary of T3.1–T3.4, test results, the rationale for the `content_late: 0.0 → 0.25` default change, link to `claudedocs/qdrant-audit-findings.md` for context
- CI checks reported (Rust build + tests)
- **PR is NOT merged**; the goal must explicitly say "ready for team review" and stop
- Constraint #5 enforced — squashing is also forbidden; the eventual merge uses `--merge`

#### Phase 4C · Docker server variant regression (must pass before opening PR)
**Purpose**: prove engine changes work in the Docker all-in-one server variant, not only the desktop.

**Gate (passes when)**:
- `cargo check --manifest-path src-tauri/Cargo.toml --features web` → 0 errors
- `docker build -t memex-allinone-test deploy/web/` succeeds (image builds)
- (if engine runnable locally) `docker run -p 8765:8765 memex-allinone-test` boots and:
  - `curl http://localhost:8765/api/health` → 200
  - `curl http://localhost:8765/metrics` → text/plain with valid Prometheus exposition (NEW from T3.2)
  - `curl -X POST http://localhost:8765/mcp -d '{…}'` MCP smoke (existing) → tools list returns 11
- If Docker not runnable in the harness, mark deferred AND document the exact reproduction command in the PR description so team can verify

**Final self-review checklist (8 items, must surface in conversation at end)**:
1. (1) TurboQuant correction in landing + docs · 2. (2) STORE-band SVG shows 8 slots · 3. (3) Q7 + Q8 cards live · 4. (4) qd-bullets 6→13 · 5. (5) Both interactive demos live · 6. (6) Engine PR opened, NOT merged, all tests pass · 7. (7) Docs synced (T5.1–T5.5) · 8. (8) Docker server variant builds + boots + serves all endpoints including new /metrics

---

## §2 · Atomic task list (27 tasks)

(Original Tier 3 had 10 items; the audit collapsed 6 of them into "already active, just surface them" which moved to Tier 2 &'s "activate dormant features" into "they're already on, just surface them properly".)

### Tier 1 · Accuracy fixes (4 tasks)

| ID | Task | File(s) | Acceptance check |
|---|---|---|---|
| **T1.1** | Replace ALL "binary-quantized HNSW" / "BQ" wording on landing with **"TurboQuant bits-2 + 2× oversampling + rescore"** | `index.html` Q1 card body · arch SVG STORE-band pill `5 × 384-D · COSINE · BINARY-QUANTIZED HNSW PER VECTOR` · qd-extras bullet "Binary-quantized HNSW per named vector" | `grep -c "BINARY-QUANTIZED\|binary-quantized" index.html` == 0; `grep -c "TurboQuant\|turbo-quant" index.html` ≥ 4 |
| **T1.2** | Architecture STORE-band SVG: extend "5 named vectors per point" → 5 dense + 2 sparse (IDF) + 1 multivector (`content_late`, MaxSim). Add visible chips for sparse + multi; label the multi as "rerank-only (off by default)" if T3.3 hasn't yet activated it | `index.html` arch SVG (the schema-panel rect at viewBox ~600-810) | viewBox holds the 3 extra slots without clipping; `grep -E "path_sparse|tool_sparse|content_late" index.html` ≥ 3 inside the arch block |
| **T1.3** | Audit every Q1-Q6 card body for code-reality drift. Known issues: Q1 (T1.1 covers), Q4 caption "MST" wording (server returns sampled K-NN pairs, MST built client-side via `petgraph`). Add a clarifying sub-line if needed | `index.html` Q1, Q4 | each Q card claim re-derived from grep; Q4 mini-viz label updated |
| **T1.4** | Sync `docs/qdrant-features.md` to v3 reality — currently describes v2. Add sections for: TurboQuant, per-lens HNSW, sparse+multivector slots, Formula prefetch + RRF + MMR + RelevanceFeedback, is_tenant, datetime index | `docs/qdrant-features.md` | `grep -cE "v3\|TurboQuant\|content_late\|path_sparse" docs/qdrant-features.md` ≥ 8 |

### Tier 2.B · Small-things bullets expansion (9 tasks — 6→13 bullets, but T2.4 also touches T1.1)

| ID | Bullet text |
|---|---|
| **T2.4** | **TurboQuant bits-2 + 2× oversampling + rescore** — "2-bit per dim · 2× oversampling guard · rescore on. The compression is real; the accuracy holds." (replaces existing BQ bullet — coordinated with T1.1) |
| **T2.5** | **Per-vector HNSW tuning** — "content m=24/ef=200 · code m=20/150 · error m=16/100 · tool & path m=12/64 — each lens gets its own graph density" |
| **T2.6** | **Server-side `group_by`** — "One query returns top-K per project — no client-side bucketing" |
| **T2.7** | **Tenant-flagged keyword index** — "`is_tenant: true` on `project_name` — Qdrant optimizes the field as a partition key" |
| **T2.8** | **Datetime payload index** — "`start_ts_dt` via `DatetimeIndexParamsBuilder` — recency queries first-class" |
| **T2.9** | **Full-text payload index** — "`ai_title` indexed with `FieldType::Text` — lexical search on session titles" |
| **T2.10** | **Strict-mode resource caps** — "85% RAM cap + 100-point query cap — embedded Qdrant can't OOM your laptop" |
| **T2.11** | **`SetPayload` + `HasIdCondition`** — "Payload-only updates skip the embedder; known-set re-rank without a full search" |
| **T2.12** | **`OrderBy` recency lane** — "`OrderBy { start_ts_dt, DESC }` for the recents panel — server-sorted" |

Acceptance: `grep -c '<li><b>' index.html` inside `.qd-bullets` grows from 6 to 13.

### Tier 2.A · New Q-cards (Q7 + Q8) (3 tasks)

| ID | Task | File(s) | Acceptance check |
|---|---|---|---|
| **T2.1** | **Q7 card — Server-side scoring (Formula · Prefetch · RRF · MMR · RelevanceFeedback)**. Body cites `retrieval.rs::FormulaBuilder` + `DecayParamsExpressionBuilder` + `RrfBuilder` + `MmrBuilder` + `PrefetchQueryBuilder` + `Query::RelevanceFeedback`. Mini-viz: animated 3-stage pipeline (prefetches → Formula fusion → MMR diversify) + small RelevanceFeedback chip | `index.html` `#qdrant .qd-grid` | `grep -c 'data-q="Q7"' index.html` == 1; mini-viz contains a 3-stage diagram; body mentions `Query::new_formula` |
| **T2.2** | **Q8 card — Hybrid retrieval (dense + sparse IDF + ColBERT MaxSim)**. Body cites `schema.rs::SPARSE_VECTORS` + `MULTIVECTOR_NAME` + `lens.rs::active_sparse_specs`. Mini-viz: 3 rails (dense / sparse / multi) feeding into a unified ranked list. Explicitly says "5 dense + 2 sparse ACTIVE by default; 1 multivector rerank slot opt-in" (or, if T3.3 ships first, "all 8 slots active by default") | `index.html` `#qdrant .qd-grid` | `grep -c 'data-q="Q8"' index.html` == 1; mini-viz shows 3 lanes converging; body mentions `path_sparse`, `tool_sparse`, `content_late` |
| **T2.3** | Both new cards get entry animations + hover details matching Q1-Q6 (`.qd::after` radial glow, stagger `transition-delay`, `.qd-xref` cross-link chip pointing to relevant architecture-section anchor) | `index.html` `<style>` + new card markup | both cards animate on view; both have a `.qd-xref` chip; `grep -c 'class="qd-xref"' index.html` ≥ 8 (was 6) |

### Tier 2.C · Interactive demos (2 tasks)

| ID | Task | File(s) | Acceptance check |
|---|---|---|---|
| **T2.13** | **RelevanceFeedback playground** — 5 mock result cards in a card row after Q2's Q-grid. 👍 / 👎 toggle buttons per card; on submit, compute a mock new rank ordering and animate cards to their new positions with `transform: translateY`. Show the `FeedbackItem` JSON (mock) in a side panel | `index.html` `#qdrant` after Q2 playground | `grep -c 'id="qd-feedback-pg"' index.html` == 1; JS handles `data-feedback-card`; running the demo updates a visible ranking list |
| **T2.14** | **Hybrid lane visualizer** — inside the Q8 card body, 3 toggles (Dense / Sparse / Late) each show or hide their contribution to a shared 8-row mock result list. Result rows re-order with `transform` transitions when toggled | `index.html` Q8 card body | `grep -c 'id="qd-hybrid-viz"' index.html` == 1; 3 toggles exist; results animate on toggle change |

### Tier 3 · Engine activation (4 tasks)

| ID | Task | File(s) | Acceptance check |
|---|---|---|---|
| **T3.1** | **Facets in Wrapped** — replace `wrapped.rs::scroll_window` + client tally with `Qdrant::facet(...)` calls for `project_name`, `git_branch`, `intent`, `outcome`, `source_agent` fields. Keep a scroll fallback for not-faceted aggregates (per-day error count). SDK 1.18.0 ships `Facet` (confirmed at `~/.cargo/registry/src/.../qdrant-client-1.18.0/tests/snippet_tests/test_facets.rs`) | `src-tauri/src/wrapped.rs` | `cargo test wrapped` passes; one of the new tests asserts the Facet path is hit. Optional perf assertion: assembly time on a 1000-session fixture < 200ms |
| **T3.2** | **Prometheus `/metrics`** in server variant — add `/metrics` route to `web.rs` exposing: `memex_queries_total`, `memex_recall_polls_total`, `memex_embedder_lock_waits_seconds`, `memex_snapshot_bytes`, `memex_points_indexed_total`, `memex_errors_recalled_total` | `src-tauri/src/web.rs` (use `prometheus` crate or hand-rolled emitter) · `Cargo.toml` if needed | `curl http://localhost:8765/metrics` returns valid Prometheus 0.0.4 text exposition with ≥ 6 metric families |
| **T3.3** | **Default-weight tuning** — `LensWeights::default()` currently has `content_late: 0.0`. Set it to a deliberate non-zero default (proposed: 0.25) so the ColBERT late-interaction lane is ON by default without dominating. Verify via `eval_ndcg.rs` fixture | `src-tauri/src/lens.rs::LensWeights::default()` + (optional) `src/main.js` slider initial values | `LensWeights::default().content_late > 0.0`; existing eval_ndcg fixture passes or improves recall@10 |
| **T3.4** | **Custom analyzer research** — investigate whether `qdrant-client::FieldType::Text` exposes tokenizer knobs (camelCase / snake_case splitter) for `ai_title`. Apply if supported with a passing schema test; otherwise document the SDK limit in `docs/wired-but-dormant.md` (T5.2) | `src-tauri/src/schema.rs` (potentially) · `docs/wired-but-dormant.md` (definitely, either way) | either field index has explicit tokenizer config with a passing test, OR doc explicitly states the limit + cites the 1.18 SDK source path |

### Tier 5 · Documentation (5 tasks)

| ID | Task | File(s) | Acceptance check |
|---|---|---|---|
| **T5.1** | Sync `docs/qdrant-features.md` to v3 (also covered partially by T1.4 — coordinate to avoid double-write) | `docs/qdrant-features.md` | covers TurboQuant, per-lens HNSW, sparse+multivector slots, Formula+RRF+MMR+Prefetch, RelevanceFeedback, is_tenant, datetime, strict-mode |
| **T5.2** | Create `docs/wired-but-dormant.md` — honest list of features in code with status flags. After T3.3 most things will be ON; this doc captures whatever remains dormant (e.g., custom analyzer if T3.4 doesn't ship) | `docs/wired-but-dormant.md` (new) | file exists; lists each item with status `wired:on \| wired:off \| not-wired` and a short rationale |
| **T5.3** | Create `docs/benchmarks.md` — recall@10, latency-p95, index-size comparing baseline f32 vs TurboQuant bits-2 vs TurboQuant bits-2 + 2× oversampling + rescore. Numbers from a reproducible fixture run | `docs/benchmarks.md` (new) · benchmark script in `scripts/` if needed | file has a ≥3-row table + the command to reproduce |
| **T5.4** | Add 3 sequence diagrams to `docs/architecture.md`: (a) **index path** (parser → embed → upsert), (b) **query path** (lens weights → prefetch chain → Formula fusion → MMR diversify → result), (c) **snapshot lifecycle** (POST → file → GET → restore) | `docs/architecture.md` | 3 sequence diagrams present (ASCII or mermaid) and labeled with their phase headings |
| **T5.5** | "30-day adoption" credibility callout on landing — short pill near the Qdrant section bottom: "Qdrant 1.18 features adopted within 30 days of release: 4 (TurboQuant · RelevanceFeedback · is_tenant index · Datetime index)" | `index.html` near the Qdrant section bottom (or in the qd-extras block) | the callout renders and the count is provable from grep'able schema constants |

---

## §3 · `/goal` condition (copy-paste-ready)

The exact text below is meant for `/goal` invocation. It encodes the 5-phase workflow.

```
END STATE — Two release tracks finished honestly per user decision #5:
TRACK A (LANDING) is fully DEPLOYED to https://two-weeks-team.github.io/memex/.
TRACK B (ENGINE) stops at an OPEN PR (not merged) awaiting team review. Both
tracks reflect honest Qdrant 1.18 capability and both work for the desktop
(Tauri) AND the Docker all-in-one server variant. Reached through five gated
phases: (P0) AUDIT — re-verify claudedocs/qdrant-audit-findings.md §9 checklist
A-J against current code/registry; reconcile any drift before any other work.
(P1) PLAN — build a TodoWrite task list covering all 27 atomic tasks from
claudedocs/qdrant-improvement-goal.md §2. (P2) IMPLEMENT — execute every task
in the dependency-safe order in §1 Phase 2. (P3) VERIFY — run skill validator,
node --check, python html.parser balance, cargo check (with and without
--features web), cargo test, on the final state; every CHECK line surfaced in
conversation. (P4) RELEASE — split: P4A landing gh-pages direct deploy with
live curl + grep proves new content + /forge preserved + deploy commit hash;
P4B engine PR opened against main (NOT merged) with full body referencing
this goal + the audit findings; P4C Docker server variant regression (build,
boot, /api/health, /metrics, /mcp tools/list smoke) before opening the PR.
Final self-review surfaces 8 DONE items.

Concretely END STATE means: (1) all "binary-quantized" wording on landing + docs
replaced with "TurboQuant bits-2 + 2× oversampling + rescore"; (2) architecture
STORE-band SVG shows 5 dense + 2 sparse + 1 multivector slots (8 total) with
honest dormancy labels; (3) two new Q-cards Q7 (Server-side scoring · Formula +
Prefetch + RRF + MMR + RelevanceFeedback) and Q8 (Hybrid retrieval · dense +
sparse + late-interaction) with mini-vizs + entry animation + hover detail + 
.qd-xref cross-ref chip; (4) qd-bullets expanded from 6 to 13; (5) two new
interactive demos — RelevanceFeedback playground (5 mock cards · 👍/👎 rank
recompute · live FeedbackItem JSON) and Hybrid lane visualizer (3 toggles
re-order results) — both with stagger entry and reduced-motion fallback; (6)
engine PR open (NOT merged) containing: Facets API replacing scroll+tally in
wrapped.rs; /metrics Prometheus endpoint shipped on Docker server variant
(deploy/web/) with ≥6 metric families; LensWeights::default() content_late
changed from 0.0 to a deliberate non-zero (~0.25); custom-analyzer investigation
done (applied OR documented as a 1.18 limit); Docker image builds with engine
changes and exposes the new /metrics; (7) docs: docs/qdrant-features.md synced
to v3, docs/wired-but-dormant.md created, docs/benchmarks.md created with 3-row
table, docs/architecture.md gains 3 sequence diagrams, landing gains the
"30-day adoption: 4 features" callout.

PHASE 0 AUDIT (MUST surface every line in conversation) —
- run the 10 grep commands from claudedocs/qdrant-improvement-goal.md §1 Phase 0
- report each result against the expected value in claudedocs/qdrant-audit-findings.md §9
- if any drift, append a "§12 drift since 2026-05-28" section to qdrant-audit-findings.md
- pass the gate only when all 10 items confirmed (with or without reconciled drift)

PHASE 1 PLAN (MUST surface) —
- emit a TodoWrite list with one item per task from §2 (23 tasks; ID, file, acceptance check)
- order them per §1 Phase 2

PHASE 2 IMPLEMENT — execute each task; update TodoWrite as you go.

PHASE 3 VERIFY (MUST surface every line) —
- skill validator on index.html → ends "PASS: index.html"
- node --check on extracted inline JS → no error
- python html.parser tag-balance → balanced
- grep -c "BINARY-QUANTIZED\|binary-quantized" index.html → 0
- grep -c "TurboQuant\|turbo-quant" index.html → ≥ 4
- grep -oE 'data-q="Q[1-8]"' index.html | sort -u | wc -l → 8
- grep -c 'class="qd-xref"' index.html → ≥ 8
- grep -c '<li><b>' index.html in qd-bullets region → 13
- grep -c 'id="qd-feedback-pg"' index.html → 1
- grep -c 'id="qd-hybrid-viz"' index.html → 1
- arch SVG references for path_sparse + tool_sparse + content_late → ≥ 3
- cd src-tauri && cargo check → 0 errors
- cd src-tauri && cargo test → all pass (specifically the new wrapped + lens tests)
- curl http://localhost:8765/metrics → text/plain Prometheus exposition with ≥6 metric families (skip if engine not running locally; mark as deferred)
- grep "content_late: 0.0" src-tauri/src/lens.rs in LensWeights::default() block → 0
- docs/qdrant-features.md grep TurboQuant\|content_late\|path_sparse → ≥6
- docs/wired-but-dormant.md exists; docs/benchmarks.md exists with a comparison table; docs/architecture.md contains 3 sequence diagrams

PHASE 4 RELEASE — split into 4A landing-deploy, 4B engine-PR, 4C docker-regression.

PHASE 4A landing deploy (MUST surface) —
- after gh-pages push: curl -s https://two-weeks-team.github.io/memex/ | grep -c "TurboQuant" → ≥1; HTTP 200; /forge gallery still 200
- show the gh-pages deploy commit hash

PHASE 4B engine PR (MUST surface) — STOP HERE for engine, do NOT merge.
- engine changes pushed to feat/qdrant-uplift (or chosen base off origin/main)
- `gh pr create` opened (draft or ready) against main with body referencing claudedocs/qdrant-improvement-goal.md + claudedocs/qdrant-audit-findings.md
- PR body summarizes T3.1–T3.4 + test results + content_late default change rationale
- CI checks pass (Rust build + tests); show URL + status
- The conversation EXPLICITLY says "engine PR opened — awaiting team review — DO NOT merge from this goal" — never auto-merge

PHASE 4C docker regression (MUST surface; runs BEFORE opening engine PR) —
- cargo check --manifest-path src-tauri/Cargo.toml --features web → 0 errors
- docker build -t memex-allinone-test deploy/web/ → image builds (smoke)
- (if Docker runnable locally) docker run -p 8765:8765 memex-allinone-test; then:
  - curl http://localhost:8765/api/health → 200
  - curl http://localhost:8765/metrics → valid Prometheus exposition (T3.2)
  - curl -X POST http://localhost:8765/mcp -d '{"jsonrpc":"2.0","method":"tools/list","id":1}' → tools list with 11 entries
- if not runnable locally, mark deferred AND put exact repro commands in the PR body so team can verify

Final self-review (MUST surface, 8 items) —
1. TurboQuant correction landed in landing + docs
2. STORE-band SVG shows 8 slots (5 dense + 2 sparse + 1 multi) with honest labels
3. Q7 + Q8 cards live with mini-vizs, entry animation, hover detail, cross-ref
4. qd-bullets 6→13
5. Both interactive demos live (RelevanceFeedback playground + Hybrid lane visualizer)
6. Engine PR opened, NOT merged, cargo check + tests pass, Docker server regression OK or deferred-with-repro
7. Docs synced (qdrant-features.md, wired-but-dormant.md, benchmarks.md, architecture.md, "30-day adoption" callout)
8. All existing interactivity preserved (14 arch hover, Q1-Q6 motion, topology galaxy, recall typewriter, hero parallax, MCP filter, lens glyphs, mix/agent vizs, safety icons)

CONSTRAINTS —
- LANDING (index.html + docs/) ships via direct gh-pages deploy as in the previous round.
- ENGINE (src-tauri/, Cargo.toml, deploy/web/) ships ONLY to OPEN PR. NEVER merge. The eventual merge requires team review and uses --merge (NEVER --squash).
- DOCKER SERVER VARIANT is a 1st-class target. Every engine change must work for both desktop (Tauri) and Docker (deploy/web/ → :8765). The new /metrics endpoint specifically lives in the Docker variant.
- index.html stays one self-contained file (inline CSS + vanilla JS, no build/CDN/framework)
- honest copy: every claim must match grep-able code; no invented numbers; label illustrative scores in demos
- back up index.html before each deploy (/tmp/memex-index-*.bak.html); never delete /forge or docs on gh-pages
- Rust code must cargo check cleanly and pass existing tests; new tests added for new behavior (Facets, /metrics, default weights, custom analyzer outcome)
- preserve ALL current interactivity: 14 architecture hover wrappers, Q1-Q6 motion classes, topology galaxy hover tooltip, recall typewriter loop, hero parallax, MCP category filter, lens glyphs, mix/agent mini-vizs, safety SVG icons — each must still pass its grep check after the work
- prefers-reduced-motion still respected on all new animations
- Qdrant v1.18.1 pin in deploy/web/docker-compose.yml unchanged unless explicitly justified
- do NOT proceed past Phase 0 if any audit item A-J drifted in a way that invalidates this goal's tasks — reconcile first
- if engine changes can't be tested live (no local Qdrant container running), cargo check + cargo test still mandatory; defer the curl smoke checks with explicit deferral note (don't silently skip)

CAP — stop after 200 turns, or when Phase 4 final self-review (8 items) is
fully DONE and Phase 4A live verification passes AND Phase 4B engine PR is
open (not merged), whichever comes first.
```

---

## §4 · `/goal-start` decision sheet

When `/goal-start` runs in the next session:

| `/goal-start` prompt | Pre-locked answer |
|---|---|
| Turn cap | **200 turns** |
| Landing deploy path | **gh-pages direct deploy** (current orphan-worktree method) — Tier 1 + 2 + 5 docs |
| Engine release path | **OPEN PR only — DO NOT MERGE** (per user decision #5). Branch: **`feat/qdrant-uplift`** new branch off `origin/main` |
| Docker server variant | **1st-class target** (per user decision #6). Engine PR must include Docker build smoke + (optional) live `/metrics`, `/api/health`, `/mcp` regression |
| Engine breaking risk | Default-weight change (T3.3) is the only behavior-affecting one; mitigate with `eval_ndcg` fixture check before commit + Docker boot regression |
| Estimated turns | ~75–115 of the 200-turn cap |

---

## §5 · Out of scope (explicitly NOT in this goal)

Per Tier 4 of the proposal:

- Recommend API alternatives to Discovery
- `UpdateVectors` API for embedder swaps
- Collection aliases for blue-green migration
- Multi-collection support
- Geo filter
- Optimizer tuning (vacuum, indexing threshold)
- API key / JWT / TLS authentication (server variant uses Caddy)

These re-evaluate in a future round.

---

## §6 · Related artifacts

| Path | Purpose |
|---|---|
| `claudedocs/qdrant-feature-comparison.md` | Initial code-vs-Qdrant matrix |
| `claudedocs/qdrant-improvement-plan.md` | 5-tier, 36-item proposal |
| `claudedocs/qdrant-audit-findings.md` | **1st-pass AUDIT FACT baseline — Phase 0 re-verifies this** |
| `claudedocs/qdrant-improvement-goal.md` | **THIS doc — the audit-first phased `/goal` spec** |
| `index.html` § `#qdrant` | Current state of the Qdrant landing section |
| `docs/qdrant-features.md` | Engineer's tour (partially v2-stale; T1.4 + T5.1 fix) |
| `src-tauri/src/{schema,lens,indexer,retrieval,wrapped,web,crud}.rs` | Code — the truth |

---

## §7 · Next-session checklist

When the next session starts (via `/handon` or `/goal`):

1. ✅ Load this doc and `qdrant-audit-findings.md` (the FACT baseline)
2. ⏳ Run Phase 0 AUDIT — surface all 10 grep commands' output
3. ⏳ Confirm gate OR reconcile drift
4. ⏳ `/goal-start` with the text in §3 → `/goal`
5. ⏳ Proceed through Phase 1–4 with cap 200 turns
6. ⏳ Deploy + final report
