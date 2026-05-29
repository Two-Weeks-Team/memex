# Session Handoff — 2026-05-28

> 이전: `claudedocs/2026-05-27-session-handoff-3.md` (3 PR merged · WOW gallery 라이브 · 스킬 재현 검증)

---

## §0 두 줄 요약

- **비기술자용**: 이번 세션은 랜딩의 hero · architecture · qdrant · topology · recall · 거의 모든 섹션을 인터랙티브 다이어그램 + 애니메이션 + 호버 상세 + 인터랙티브 playground로 끌어올려 배포했고, 마지막으로 **Qdrant 섹션의 정직성 audit**을 수행해 "랜딩이 코드 대비 2.2× 저평가 중"이라는 사실을 발견. 다음 세션에서 `/goal`로 audit-first 5-phase 작업을 돌릴 준비를 마쳤다.
- **다음 세션 1순위 액션**: `/handon` → `claudedocs/qdrant-improvement-goal.md` 의 §3 텍스트를 `/goal`에 붙여 5-phase (AUDIT → PLAN → IMPLEMENT → VERIFY → DEPLOY) 실행.

---

## §1 진행한 작업 (시간순)

### Phase A — `/handoff` 인계 후 새 hero 카피 + /goal 실행
- 사용자 지시: hero를 "memex — Sense It, Spark Memory. / Turn every moment into living trails of meaning, connection, and intelligence."로 고정
- `/goal-start` 100 turns + gh-pages 직접 배포 확정
- 첫 randing 재구성 배포(`feat/landing-v2 @ 48ea734` → gh-pages `9bd013e`): constellation hero · 5 lens · 11 MCP 도구 · 인라인 SVG 아키텍처 다이어그램 · 5-band 구조

### Phase B — Qdrant 섹션 6 primitive 추가
- Q1..Q6 + small-things 6 bullet 추가 (gh-pages `72ce42c`)
- 5 named-vector → BQ-HNSW · 11 MCP tools · API 호출문 + 코드 포인터 + 크로스 링크

### Phase C — 섹션 순서 + hero typography 리팩터
- 사용자 결정: foundation-first 순서 · wordmark + 거대 태그라인 분리 (gh-pages `5175ddb`)
- 새 순서: Hero → Intro → **Architecture → Qdrant** → 5 Lenses → Topology → Mix → Recall → Agents → MCP → Run → Safety

### Phase D — Architecture diagram 베스트 프랙티스 재설계
- viewBox 1400 × 1480 · 6 레이어 밴드 (01 SOURCES → 06 SURFACES) · keystone STORE (gold) · 5 named-vector chips + payload schema + indexed payload keys panel · 11 MCP tools categorized · 4 host tiles with custom glyphs (gh-pages `1ee4c56`)

### Phase E — Architecture 인터랙티비티 (entry animation + 14 hover details)
- IntersectionObserver entry animation (SVG fade + stroke-dashoffset arrow draw)
- 14 SVG 그룹 hover/focus 상세 다이얼로그 (5 lens chips + 5 capabilities + 4 hosts)
- 다이얼로그가 *요소에서 자라나는* 자연 expansion 모션 (transform-origin 보정)
- ESC / scroll / resize 닫기 · 키보드 Tab traversable (gh-pages `5e60ebf` → `2f1c054`)

### Phase F — Qdrant 섹션도 같은 수준
- Q1..Q6 각자 mini-viz SVG · Q2는 인터랙티브 lens-slider playground (실시간 Σ(s·w)/Σw) · 카드 stagger 진입 · cross-ref chips (gh-pages `248ad95`)

### Phase G — 모든 다른 섹션 + Qdrant 추가 임팩트
- Universal `[data-stagger]` + hover lift · hero scroll-cue + mouse parallax · lens glyphs (5) · mix mini-vizs (3) · agent mini-vizs (3) · safety SVG icons (3) · MCP category filter (interactive 11→4 tools) · Qdrant Q1-Q6 deep motion (q1bar scale-in · q3pair draw · q4mst sequential · q5pipe pulse · q6step sequential) · Q2 result number recompute pop (gh-pages `7fb1b81`)

### Phase H — Topology + Recall 풀-리디자인
- Topology: 33-node JS-rendered 갤럭시 · 31 MST edges · ambient drift · 호버 시 halo + 이웃 강조 + tooltip with project/age/subject/degree · 5 project colors + legend (gh-pages `b6c7cfc`)
- Recall: typewriter 루프 · 3 mock 사이클 · 6-step flow column · 12s polling pulse 인디케이터 · 3 context cards

### Phase I — Qdrant 비교 audit (현재)
- `claudedocs/qdrant-feature-comparison.md` 생성 — 코드가 사용하는 24 Qdrant 기능 vs 랜딩 surface 12 = **2× 저평가**
- 가장 충격적인 3건: (1) "binary-quantized" → 실제는 TurboQuant bits-2 (2) sparse + multivector 슬롯 wired (3) 1.18 신기능 4개 채택

### Phase J — 개선 제안 + decision
- `claudedocs/qdrant-improvement-plan.md` 생성 — 5-tier · 36-item 제안
- 4 AskUserQuestion으로 사용자 결정 lock: 풀 스코프 / 엔진 전부 동시 / 데모 둘 다 / docs 전부

### Phase K — Audit-first goal 문서화 (현재 — 실행 X, 문서만)
- 사용자 추가 지시 (시간순):
  - "이 결정사항을 포함하여 문서화를 모두 진행"
  - "goal에서도 가정을 모두 AUDIT하고 나서 진행"
  - "AUDIT 우선 → 계획 → 구현 → 검증 → 완전한 랜딩 구현"
  - **"랜딩은 직접 디플로이까지 진행하되, 기능구현은 PR까지만 진행. (팀원과 회의후 병합해야함)"** → User decision #5 추가
  - **"도커로 가동하는 웹서비스까지 놓치면 안되는거 알지?"** → User decision #6 추가
- **deeper 1차 audit 수행** → `claudedocs/qdrant-audit-findings.md` (FACT baseline 11 sections)
- **`claudedocs/qdrant-improvement-goal.md` 재구성** → 5-phase workflow:
  - P0 AUDIT (10-item checklist 재검증)
  - P1 PLAN (TodoWrite 27 atomic tasks)
  - P2 IMPLEMENT (10단계 dependency-safe order)
  - P3 VERIFY (validator + node + html.parser + `cargo check` (with & without `--features web`) + `cargo test`)
  - P4 RELEASE — split:
    - **P4A 랜딩 직접 deploy** (gh-pages, Tier 1+2+5 docs)
    - **P4B 엔진 PR ONLY (no merge)** — `feat/qdrant-uplift` 새 브랜치 · `gh pr create` 후 STOP
    - **P4C Docker 서버 변형 regression** (`cargo check --features web` · `docker build deploy/web/` · `/api/health` · `/metrics` · `/mcp` smoke)
- 8-item 최종 self-review checklist

---

## §2 현재 상태

### Git / PR
| 항목 | 상태 |
|---|---|
| 작업 브랜치 | `feat/landing-v2 @ 49ea015` (마지막 커밋: topology + recall full rebuild) |
| origin/main | `e691834` (3-day-old, PR #6/#7/#8/#9 모두 병합됨) |
| 추적 안 됨 (working tree) | `M src-tauri/src/{cli,companion}.rs` (이전 세션 변경) · `M .gitignore` |
| gh-pages tip | `b6c7cfc17977ac6ea15afa3601b4150c9186d2fe` (topology + recall 배포) |
| 열린 PR | (마지막 확인) #10 docs(readme) sync — sgwannabe |

### Live URLs (확인됨, HTTP 200)
- 랜딩 루트: `https://two-weeks-team.github.io/memex/`
- WOW 갤러리: `https://two-weeks-team.github.io/memex/forge/` (66개 mockup 보존)

### 현재 랜딩 인터랙티비티 (전체 확인 가능)
- Hero: constellation canvas · scroll-cue · mouse parallax
- Architecture: 14 hover 상세 다이얼로그 (자연 expansion) · entry animation
- Qdrant: 6 카드 deep motion · Q2 인터랙티브 슬라이더 + recompute pop · cross-refs · stagger
- Lenses: 5 lens glyphs · 슬라이더 bar fill 애니메이션 · hover lift
- Topology: 33-node 갤럭시 · ambient drift · hover halo + tooltip
- Mix · Agents · Safety: 미니-viz + stagger + hover lift
- Recall: typewriter 루프 · 6-step flow sync · 12s 폴링 펄스
- MCP: 11 tools + 카테고리 필터 (interactive 7 chips)

### 다음 세션이 사용할 4개 핵심 문서 (이 세션에서 생성)
| 파일 | 역할 |
|---|---|
| `claudedocs/qdrant-feature-comparison.md` | 코드 vs Qdrant 전체 매트릭스 (초기 분석) |
| `claudedocs/qdrant-improvement-plan.md` | 5-tier · 36-item 제안서 |
| **`claudedocs/qdrant-audit-findings.md`** | **1차 AUDIT FACT baseline — 11 섹션 · 10개 checklist** |
| **`claudedocs/qdrant-improvement-goal.md`** | **5-phase `/goal` spec · 27 atomic tasks · §3 copy-paste-ready** |

### 환경
- node v24.15.0 · python3 3.12.4 · Rust crate `qdrant-client = "1"` 해석된 버전 1.18.0 · Qdrant server pin `v1.18.1`
- Skill validator · node --check · python html.parser tag-balance 전부 PASS 상태로 마지막 deploy

---

## §3 다음 세션에서 할 수 있는 것

### 즉시 가능 (외부 의존 없음)
1. **`/goal`로 Qdrant uplift 풀 패스 실행** — `qdrant-improvement-goal.md` §3 텍스트 그대로 `/goal`에 붙이면 5-phase 자동 진행
2. **Phase 0 AUDIT 단독 실행** — `qdrant-improvement-goal.md` §1 Phase 0의 10 grep 명령 직접 실행해서 baseline 재확인만 먼저
3. **이전 라운드 작업 검토** — Architecture · Qdrant · Topology · Recall 라이브 직접 확인 (URL 위)

### 사용자 입력 필요
- (Phase 4 시점) 라이브 배포 직전 변경사항 최종 확인
- T3.3 `content_late` 기본값 — 제안 0.25; eval_ndcg 결과 보고 결정 가능
- T3.4 custom analyzer — SDK 1.18 미지원 시 deferred로 문서화할지

---

## §4 할 수 없는 것 (외부 변수)
- 다른 팀원의 PR 진행 / 머지 — 외부 작업
- Qdrant 1.18+ SDK 업스트림 변경 — 외부 의존
- 해커톤 심사 일정 D-0 = 2026-06-01 (D-4)

---

## §5 추가로 필요한 것
- (선택) main 브랜치에 `feat/landing-v2`를 PR로 머지할지 결정 — 현재 모든 랜딩 + 문서 작업은 `feat/landing-v2`에 있고 gh-pages만 직접 배포되어 main과 분리됨
- (선택) `feat/qdrant-uplift` 브랜치를 origin/main에서 새로 따서 goal 진행할지, `feat/landing-v2`에 이어 진행할지

---

## §6 다음 세션 시작 프롬프트 (복사용)

```text
/handon

이전 세션 핸드오프: claudedocs/2026-05-28-session-handoff.md

읽고 다음 결정 사항에 답한 뒤 진행하세요:
1. Qdrant uplift를 지금 /goal로 실행할까요? (5-phase 200-turn)
   → claudedocs/qdrant-improvement-goal.md §3 텍스트를 /goal에 그대로 붙여 실행
2. 새 브랜치 feat/qdrant-uplift를 origin/main에서 따서 진행? 아니면 feat/landing-v2 위에서 계속?
3. Phase 0 AUDIT만 먼저 단독 실행 후 결과 확인하고 진행할까요? 아니면 5-phase 한 번에?
4. 팀원 PR #10 (README sync) 검토 후 머지할까요?

제약 (locked from 2026-05-28 session):
- 랜딩 (Tier 1+2+5 docs): 직접 gh-pages deploy
- 엔진 (Tier 3, src-tauri/): OPEN PR까지만, 절대 병합 금지 (팀원 리뷰 필요)
- Docker 서버 변형 (deploy/web/, :8765): 1급 시민 — engine 변경은 desktop + Docker 둘 다 동작 필수
- 병합은 --merge만 (squash 금지) · :8080/.env 제약 해제 (memex 한정)
- 외부 작업 사전 보고 · /forge 갤러리 보존

D-day: 2026-06-01 (D-4)
```

---

## §7 핵심 자산 위치 reference

| 자산 | 경로 |
|---|---|
| **Audit-first goal spec** | `claudedocs/qdrant-improvement-goal.md` (5-phase · 23 tasks · §3 `/goal` 텍스트) |
| **1차 AUDIT 결과 (FACT baseline)** | `claudedocs/qdrant-audit-findings.md` (11 섹션 · 10-item checklist) |
| **개선 제안서 (전체)** | `claudedocs/qdrant-improvement-plan.md` (5-tier · 36-item) |
| **분석 매트릭스** | `claudedocs/qdrant-feature-comparison.md` |
| 현재 랜딩 소스 | `index.html` (~3334 lines · 자체 완결 · inline CSS + vanilla JS) |
| Qdrant v3 schema (truth) | `src-tauri/src/schema.rs` (5 dense + 2 sparse + 1 multivec · TurboQuant bits-2 · 10 indexed payload fields) |
| 쿼리 lane builder | `src-tauri/src/lens.rs` (active_dense_specs · active_sparse_specs · build_prefetches) |
| 영구 메모리 인덱스 | `~/.claude/projects/-Users-kimsejun-Documents-GitHub-memex/memory/MEMORY.md` |
| WOW 스킬 (포터블) | `~/.claude/skills/wow-scene-mockups/` + `.skill` 패키지 |

---

## §8 알려진 issue / open question

1. **Architecture STORE band SVG가 5 named vectors만 표시** — 실제로는 5 dense + 2 sparse (path/tool, IDF) + 1 multivector (content_late, MaxSim) = 8 slot. T1.2가 이 SVG 확장.
2. **"binary-quantized" 표기가 4곳 이상** — 실제 v3는 TurboQuant bits-2. T1.1이 일괄 정정.
3. **`content_late` ColBERT는 wired but default OFF** (`LensWeights::default().content_late = 0.0`). T3.3가 0.25로 활성.
4. **`/metrics` Prometheus 엔드포인트 없음** — `web.rs:111`는 `/api/health`만. T3.2가 새 endpoint.
5. **`wrapped.rs`가 Facets 대신 scroll+tally 사용** — SDK 1.18.0 Facet API 있음(확인). T3.1이 교체.
6. **`feat/landing-v2`가 origin/main에 머지되지 않음** — 모든 랜딩 작업이 이 브랜치에만 존재. 다음 세션에서 PR 만들지, 새 브랜치 따서 이어갈지 결정 필요.
7. **사용자 결정 6건 lock 완료** (이번 세션 — qdrant-improvement-goal.md §0):
   - #1 풀 스코프 (Tier 1+2+3+5)
   - #2 엔진 항목 전부 동시 진행
   - #3 인터랙티브 데모 둘 다 (RelevanceFeedback playground + Hybrid lane visualizer)
   - #4 docs 5건 전부 명세
   - #5 **랜딩=직접 deploy / 엔진=PR-only (팀원 리뷰 후 병합)**
   - #6 **Docker 서버 변형은 1급 시민** (engine 변경은 desktop + Docker 둘 다 통과)
8. **다음 세션 진입 시 즉시 사용 가능한 4개 핵심 문서**:
   - `claudedocs/qdrant-audit-findings.md` (1차 AUDIT FACT baseline · 10-item checklist)
   - `claudedocs/qdrant-improvement-goal.md` (5-phase goal · §3 copy-paste `/goal` 텍스트 · 27 atomic tasks)
   - `claudedocs/qdrant-improvement-plan.md` (5-tier · 36-item 제안서)
   - `claudedocs/qdrant-feature-comparison.md` (코드 vs Qdrant 매트릭스)
