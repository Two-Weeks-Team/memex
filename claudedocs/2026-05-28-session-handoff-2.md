# Session Handoff — 2026-05-28 (Part 2: Qdrant follow-up squad)

## §0 두 줄 요약

- **이번 세션**: PR #12 (Qdrant uplift)의 후속 4 issue (#13/#14/#15/#16)와 랜딩 1줄 drift까지 **6개 PR로 묶어 처리** — #17/#18/#19/#21은 머지 완료, #22/#23은 OPEN + CI CLEAN + Gemini 리뷰 1차 반영까지 끝. 자체 평가 점수 **A+ (~98)** → 두 PR 머지 후 **A++ (~99)** 도달 가능.
- **다음 세션 1순위**: 팀 리뷰 후 PR #22 + #23 머지. 머지되면 #13/#16 자동 close. 그 후 잔여 작업은 모두 deferred로 정리됨.

---

## §1 진행한 작업 (시간순)

### Phase A — PR 4건 추가 묶음 작성 (이전 세션 산출물 후속)

이전 세션 핸드오프 (`claudedocs/2026-05-28-session-handoff.md`)는 PR #12 머지 + 4개 follow-up issue 등록 (#13/#14/#15/#16) + claudedocs 보고서 2건까지 끝낸 상태. 이번 세션은 그 위에서:

| 단계 | 산출 | 상태 |
|---|---|---|
| PR #17 (`feat/qdrant-followup-priority`) | #14 + #15 + #16 Stage 1 묶음 (4 commits) | **MERGED** (Sangguen, `af3a807`) |
| PR #18 (`feat/qdrant-followup-bench`) | #13 — `MEMEX_QUANT_MODE` env + Criterion scaffold (1 commit, stacked on #17) | **MERGED** (`bfa5f77`) |
| PR #19 (`feat/qdrant-followup-landing`) | 랜딩에 PR #17/#18 변경 반영 (Q7 status · TurboQuant bullet · `ai_title_tokens` 예시 · Footer Docs links + qdrant-features link) | **MERGED** (`3f67a28`) |
| PR #21 (`fix/landing-qdrant-features-link`) | drift 1줄 cherry-pick (qdrant-features 링크가 PR #19 머지 직후 push라 #19 머지 commit에 누락됐었음) | **MERGED** (직접 `gh pr merge --merge`, `df2d78d`) |

PR #20 (Sangguen의 README rolling sync)도 같은 흐름에 머지됨 — 우리가 만든 PR은 아니지만 README가 PR #17/#18 변경에 맞춰 갱신됨.

### Phase B — Qdrant 기능 상태 페이지 v1 → v2 → v3 → v4 (gh-pages 라이브 배포)

memory의 user decision #5 ("랜딩은 직접 디플로이까지 진행") 패턴 그대로 적용 — `gh-pages` orphan worktree로 직접 deploy:

| 버전 | 라이브 | 핵심 |
|---|---|---|
| v1 (`6133267`) | https://two-weeks-team.github.io/memex/qdrant-features/ | 텍스트 분류표 (10 카테고리, 51 rows) |
| v2 (`9dac369`) | 동일 | 인터랙티브 매트릭스 — 도넛 + 필터 + 카드 그리드 + 카테고리 막대 + PR 타임라인 |
| v3 (`9ca8046`) | 동일 | 탭 추가 (Feature matrix · Deferred rationale 5 기준) + 카운트 수정 (33→35 · 51→52) |
| v4 (`bd6ca02`) | 동일 | Evaluation 탭 추가 — 4-axis 결과 + 점수 progression (96 → 98 → 99 → 100 ceiling) + POST-DEV checklist + 100 ceiling 이유 |

소스는 `claudedocs/reports/qdrant-uplift/qdrant-features-status.html` (~58 KB) — main에 commit되지 않은 상태 (untracked).

### Phase C — `/hackathon-audit` 스킬 평가 + 스킬 자체 수정

사용자 요청: "100점 아닌 이유 + `/hackathon-audit` 기준으로 평가". 4-axis 평가:

| 축 | 결과 (POST-DEV 분리 전) | 결과 (POST-DEV 분리 후) |
|---|---|---|
| 1 룰 컴플라이언스 | PARTIAL (Demo video missing) | **PASS** (Demo video → POST-DEV) |
| 2 필수 기술스택 (Qdrant) | PASS · STRONG | PASS · STRONG |
| 3 vibeDeploy 우승 플레이북 | PASS | PASS |
| 4 스토리텔링 자산 | PARTIAL (video + 7-section write-up) | **PASS** (POST-DEV로 분리) |

사용자 명시 — Demo video / Form 제출은 "개발 끝나야 찍지" → 평가에서 배제. 그에 맞춰 **스킬 자체를 수정**: `~/.claude/skills/hackathon-audit/SKILL.md`에 `Phase 3.5` (POST-DEV 분리) + Hard rules 한 줄 추가. 미래 호출부터 자동 적용.

점수 progression:
- 시작 시 (`PR 3건 OPEN`): A+ (~96)
- PR #17/#18/#19/#21 머지 후 (현재): A+ (~98)
- PR #22 + #23 머지 후: **A++ (~99)** (목표)
- 100 ceiling: structural — Qdrant upstream PR / novel algorithm / research-grade analysis 영역

### Phase D — Issue #16 Stage 2 구현 (PR #22)

사용자 명령 "구현해서 완료까지 달리면 되는거 아니야?" + "1·2순위 둘 다 진행" → MCP write tool + bench live wiring 동시 진행.

PR #22 (`feat/mcp-enrich-session`):
- 새 MCP tool `refresh_session_enrich` — 기존 enrich() 재실행 후 SetPayload (payload-only, idempotent, 재임베딩 없음)
- `WebMetrics` `pub` 노출
- `McpState`에 `#[cfg(feature="web")]` optional `Arc<WebMetrics>` field + transport-uniform `mark_indexed(n)` helper
- `new_shared_state_with_metrics(...)` constructor + `web::serve`에서 사용
- Stage 1 TODO 주석 → Stage 2 SHIPPED 노트로 교체
- `docs/wired-but-dormant.md` §C row strike-through
- 4 commits 합쳐서 1 PR (`bf1494b`)

### Phase E — 벤치 live wiring + 실측 (PR #23)

PR #23 (`feat/bench-live-wiring`):
- `benches/quant_sweep.rs` live mode stub → 실제 구현
  - 단일 tokio runtime 재사용
  - Drop + `ensure_collection_v3` (현재 `MEMEX_QUANT_MODE` 픽업) + Embedder init + bulk_index_arc
  - Criterion bench: round-robin queries → `lens_search`
  - nDCG@10 pass (not timed)
- `fixtures/labeled-queries.jsonl` placeholder 3 → **measured 12 entries** (corpus의 12 session 1:1 매칭, 1개 multi-relevant)
- `docs/benchmarks.md` "illustrative" 표 → **measured 표** (3 mode 실측: nDCG@10 0.8732 동일, latency 7.14-7.34ms, disk 1505-1735 KB)
- Honest 캡션: 작은 corpus에서 quant 압축 이득 안 보임 + production scale 1k+에서 8× 압축 + 50% latency reduction (Qdrant 블로그 인용)
- 1 commit (`f75e846`)

### Phase F — Gemini 리뷰 처리

PR #22 — Gemini HIGH security 1건 (path traversal + Codex 미지원). `sec::validate_session_path` 추가 + `codex_parser` routing 분기. commit `c1c29d9`.

PR #23 — Gemini MEDIUM 3건:
1. Hardcoded `"memex_sessions_v3"` → `memex_lib::schema::COLLECTION_V3`
2. `q_idx += 1` overflow 위험 → `q_idx = (q_idx + 1) % queries.len()`
3. Duplicate `LabeledQuery` struct → `type LabeledQuery = eval_ndcg::LabeledQuery;`

commit `6c2eb75`. CodeRabbit + Codex는 rate-limited라 실제 리뷰 없음 — Gemini가 유일하게 fact 리뷰, 모두 반영.

---

## §2 현재 상태

### Git branches

| Branch | HEAD | 상태 |
|---|---|---|
| `main` | `df2d78d` | PR #17/#18/#19/#20/#21 모두 머지된 상태 |
| `feat/mcp-enrich-session` | `c1c29d9` | PR #22 head — OPEN, CLEAN, 2 commits |
| `feat/bench-live-wiring` | `6c2eb75` | PR #23 head — OPEN, CLEAN, 2 commits |
| `feat/qdrant-followup-priority` · `feat/qdrant-followup-bench` · `feat/qdrant-followup-landing` | (머지됨) | 삭제 안전 |
| 현재 cwd | `feat/bench-live-wiring` | working tree에 untracked claudedocs/reports/* (이전 세션 잔재) |

### Live URLs (모두 200)

- https://two-weeks-team.github.io/memex/ — 메인 랜딩 (PR #19 변경 + qdrant-features link 모두 반영)
- https://two-weeks-team.github.io/memex/forge/ — 36-scene gallery
- https://two-weeks-team.github.io/memex/qdrant-features/ — v4 (3 탭: Feature matrix · Deferred rationale · Evaluation)

### Open PRs / Issues

| # | 종류 | 상태 |
|---|---|---|
| **PR #22** | refresh_session_enrich (Issue #16 Stage 2) | OPEN, CLEAN, 2 commits (`bf1494b` + `c1c29d9`) |
| **PR #23** | bench live wiring + measured (#13 closure) | OPEN, CLEAN, 2 commits (`f75e846` + `6c2eb75`) |
| Issue #16 | MCP counter Stage 2 tracking | OPEN — PR #22 머지 시 `Closes #16` 자동 close |

### CI / 검증

- `cargo check` (default + web): 0 errors
- `cargo test --lib`: **280 passed**, 0 failed
- `cargo bench --bench quant_sweep --no-run`: compiles cleanly (~28-30s)
- 두 PR 모두 4/4 standard CI SUCCESS (CodeRabbit rate-limited)
- Claude attribution audit (모든 commit + PR body): **0 hits**

### 환경

- macOS 15 · M2 / 16 GB · darwin 25.5.0
- rustc 1.93.0 (Homebrew)
- Docker Desktop 29.3.1 · `memex-qdrant` Up 2 days (127.0.0.1:6333 + 6334)
- BGE-small ONNX 캐시 — fastembed 첫 실행 시 ~130MB

### Tasks (모두 completed, 자동 정리됨)

이번 세션 7 + 6 = 13 tasks 모두 `completed`. 다음 세션은 빈 list로 시작.

---

## §3 다음 세션에서 할 수 있는 것

### 즉시 가능 (사용자 입력 없이)

- 잔여 untracked files 정리 (`claudedocs/reports/2026-05-19-upstream-divergence.html`, `claudedocs/reports/pr8/`, `claudedocs/reports/purple-oval/`, `claudedocs/reports/qdrant-uplift/artifacts/`, `claudedocs/reports/qdrant-uplift/qdrant-features-status.html` — 마지막 1개는 gh-pages 라이브 source라 main에도 commit 가치 있음)
- 머지된 brunch 정리 (`feat/qdrant-followup-priority` · `feat/qdrant-followup-bench` · `feat/qdrant-followup-landing` · `fix/landing-qdrant-features-link` — 로컬 삭제 안전)
- PR #22 / #23 슬랙 메시지 작성 (이미 직전 답변에 복붙 가능 형태로 준비됨)

### 사용자 입력 필요

- **PR #22 / #23 머지 결정** — user decision #5 ("기능구현은 PR까지만 진행, 팀원과 회의 후 병합"). Sangguen 또는 본인 판단 후 `gh pr merge --merge`
- Demo video 녹화 시점 결정 (현재 POST-DEV로 분리됨; 코드 freeze 신호 받으면 녹화 → 폼 제출)
- claudedocs HTML 보고서 4종 (`follow-up-deep-dive.html` · `follow-up-v2.html` · `qdrant-features-status.html`) 중 어떤 걸 main에 commit할지

---

## §4 할 수 없는 것 (외부 변수)

- **PR #22 / #23 자체 머지** — 사용자/팀 영역. user decision #5 룰. CI는 통과했지만 머지는 안 함
- **Demo video 녹화** — 사용자가 직접 (storyboard는 README + claudedocs 스크린샷 활용)
- **Google Form 제출** — Demo video URL이 필수 필드, 영상에 종속
- **CodeRabbit / Codex 본격 리뷰** — 두 봇 다 usage limit 초과 상태 (`chatgpt-codex-connector` "usage limits", `coderabbitai` "rate limited"). Gemini 만으로 1차 review 완료
- **Production-scale 벤치 실측** — `~/.claude/projects` 사용자 corpus 대상. fixture는 작은 corpus용. 사용자 환경에서 `MEMEX_BENCH_LIVE=1 MEMEX_QUANT_MODE=$mode cargo bench --bench quant_sweep` 실행 필요
- **100점 도달** — fundamental research (Qdrant upstream PR / novel algorithm / paper) 영역. 4일 윈도우 밖

---

## §5 추가로 필요한 것 (사용자 확인 + 환경)

| 항목 | 형태 |
|---|---|
| PR #22 / #23 머지 진행 여부 | "지금 머지" vs "팀 리뷰 더 받고" |
| Demo video 녹화 — 언제? | 코드 freeze 신호 (지금? 머지 후?) |
| claudedocs HTML 4종 main commit | 어떤 것을 commit · 어떤 것은 라이브에만 |
| Sangguen에게 슬랙 알림 | 직전 답변의 PR #22/#23 슬랙 메시지를 보낼지 |
| `MEMEX_BENCH_LIVE=1` 실측 — 사용자 corpus | `~/.claude/projects` 대상 실측 진행 의사 (선택; 도구 이미 ship됨) |

---

## §6 다음 세션 시작 프롬프트

```text
/handon

이전 세션 핸드오프: claudedocs/2026-05-28-session-handoff-2.md

읽고 다음 결정 사항에 답한 뒤 진행하세요:
1. PR #22 (refresh_session_enrich) — 지금 머지 (gh pr merge --merge)? 아니면 팀 리뷰 더 기다림?
2. PR #23 (bench live wiring) — 동일 결정
3. PR #22/#23 머지 알림용 슬랙 메시지를 추가로 보낼지 (직전 답변에 복붙 형태로 준비됨)
4. claudedocs HTML 보고서 4종을 main에 commit할지 (follow-up-deep-dive.html · follow-up-v2.html · qdrant-features-status.html · v1)
5. 머지된 follow-up 브랜치 4개 정리 (로컬 삭제) 진행 여부

제약:
- user decision #5 — 랜딩=직접 deploy / 기능구현=PR까지만 / 머지는 팀원 회의 후
- D-day: 2026-06-01 (남은 4일) — "no time limit" 룰로 품질 기준만 적용
- Demo video / Form 제출은 POST-DEV (코드 freeze 후) — 평가 점수에 영향 0
- Claude attribution은 모든 commit/PR/문서에서 영구 금지 (사용자 룰)
```

---

## §7 핵심 자산 위치 reference

| 자산 | 경로 |
|---|---|
| 이번 세션 핸드오프 (이 문서) | `claudedocs/2026-05-28-session-handoff-2.md` |
| 이전 세션 핸드오프 | `claudedocs/2026-05-28-session-handoff.md` |
| Audit baseline | `claudedocs/qdrant-audit-findings.md` (§1-§13 immutable) |
| Follow-up deep-dive v1 | `claudedocs/reports/qdrant-uplift/follow-up-deep-dive.html` |
| Follow-up alternatives v2 | `claudedocs/reports/qdrant-uplift/follow-up-v2.html` |
| Qdrant feature status (라이브 source) | `claudedocs/reports/qdrant-uplift/qdrant-features-status.html` |
| 라이브 랜딩 | `index.html` (root) — gh-pages에 직접 deploy |
| Feature status 라이브 | `/qdrant-features/index.html` on gh-pages branch |
| PR #22 head | `feat/mcp-enrich-session` @ `c1c29d9` |
| PR #23 head | `feat/bench-live-wiring` @ `6c2eb75` |
| MCP write tool 본체 | `src-tauri/src/mcp.rs::tool_call` → `"refresh_session_enrich"` arm |
| Bench harness | `src-tauri/benches/quant_sweep.rs` |
| Labeled queries fixture | `src-tauri/fixtures/labeled-queries.jsonl` (12 entries) |
| Sample corpus | `examples/sample-corpus/` (12 sessions, 4 projects) |
| 평가 표 docs | `docs/benchmarks.md` (measured 2026-05-28) |
| 자체 평가 표 | `docs/wired-but-dormant.md` (§A 23 wired / §B empty / §C 7 not-wired) |
| Hackathon audit skill | `~/.claude/skills/hackathon-audit/SKILL.md` (Phase 3.5 POST-DEV 분리 추가됨) |
| Qdrant 컨테이너 | `docker ps name=memex-qdrant` — Up 2+ days |

---

## §8 알려진 issue / open question

### Open

- **Issue #16 OPEN** — PR #22 머지 시 자동 close (`Closes #16` 키워드 명시됨). Stage 2 wire-up 완료, 마지막 한 단계 머지만 남음.
- **PR #22 / #23 머지 대기** — 팀 리뷰 (user decision #5). CI는 모두 SUCCESS, CodeRabbit/Codex는 rate-limit이라 실 리뷰 못 함 — Gemini만 1차 리뷰, 모두 반영함.
- **`MEMEX_QUANT_MODE` 환경변수의 production-scale 벤치 미진행** — 12-session corpus 기반은 끝. 1k+ 진짜 corpus 대상은 deployer 영역.

### Resolved 이번 세션에

- ~~PR #12 follow-up 4건~~ → 모두 PR로 묶임 (3건 머지 완료, 2건 OPEN)
- ~~main↔gh-pages 1줄 drift~~ → PR #21로 해소 (머지됨)
- ~~Qdrant feature 분류 시각화~~ → `/qdrant-features/` 페이지 v4 라이브
- ~~Demo video / Form 평가에 포함되어 점수 깎임~~ → 스킬 자체에 POST-DEV 분리 룰 추가
- ~~Gemini 4 findings (PR #22 HIGH + PR #23 3 MEDIUM)~~ → 모두 commit, push 완료

### Out-of-scope (intentional, 명문화됨)

- Demo video 녹화 + Form 제출 — POST-DEV (코드 freeze 후, 사용자 영역)
- Fundamental innovation (Qdrant upstream PR / novel algorithm) — 4일 윈도우 밖, 100 ceiling 영역
- MCP 추가 write tools (`tag_session` · `star_session` · `index_session_from_jsonl`) — 각 별도 보안/schema 결정 동반, 동일 wiring 패턴 따라 future PR
- Production-scale (1k+) bench 실측 — deployer 영역 (`docs/benchmarks.md` §3에 명시)
- CodeRabbit / Codex 본격 리뷰 — 두 봇 다 usage limit 초과
