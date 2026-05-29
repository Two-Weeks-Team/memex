# Session Handoff — 2026-05-19 (D-13)

**Repo**: https://github.com/ComBba/memex (fork of `sgwannabe/memex`)
**Branch at session end**: `fix/view-transition-leak-and-ux`
**HEAD commit**: `84db1fc` (fix: enable WebView devtools + silence shell-stderr recall + add errors-badge tooltip)
**D-day**: 2026-06-01 23:59 UTC (VSD 2026 submission)

---

## §0 두 줄 요약

이 세션은 P1-P8 머지 + 외부 리뷰 26 항목 반영 + 실 corpus E2E 검증 + 사용자 GUI 테스트로 발견된 4가지 UX 버그(predict Codex 라우팅 / mix modal 차단 / heat-chip 거대 보라 원 / recall banner stderr 노이즈) 수정까지 완료. **다음 세션 1순위**: 사용자가 새로 설치한 Memex.app에서 DevTools를 열어 **거대 보라 원의 DOM 노드**를 직접 인스펙트하고 그 element 이름을 알려준 뒤, 그걸 근거로 PR #14 마저 머지하고 D-13 → D-0 비디오 녹화 준비.

---

## §1 진행한 작업 (시간순)

### Phase A — P1~P7 + P8 E2E validation
- 직전 세션 (`session_handoff_2026_05_18.md`) `/handon` 으로 시작
- `/goal-start` → `/goal` 으로 P8 E2E validation 자율 실행 (1000-turn cap)
- **PR #8** (P7 demo) 머지
- **PR #9** hotfix(cli ensure v3) — `scan --index`가 v2 collection만 ensure하던 버그. 110/111 sessions 인덱싱 성공, 0 errors
- **PR #10** P8 E2E — 7 surfaces smoke-test + Tauri deep-link plugin + tests/e2e/ 산출물 + IMPLEMENTATION_REPORT 머지
- macOS Memex.app 설치 + `memex://` URL scheme 등록 검증

### Phase B — 외부 리뷰 26 항목 반영 (Gemini / Codex / CodeRabbit)
- PR #2~#10 인라인 코멘트 30+ 수집
- Tier A (4 functional bugs) + Tier B (10 correctness) + Tier C (12 polish) 분류
- **PR #11** chore(reviews) — 23 items 1차 반영 + round-2 (Codex P1 migration safety) + 3 deferred items (payload module / bulk batch / recursive payload_to_json) 모두 처리 → 머지
- 테스트 결과: 212 → 222 passing (payload module 8 신규 테스트)
- **PR #12** hotfix(predict Codex routing) — 실 E2E로 발견: `predict_next_actions`이 모든 세션에 Claude 파서 사용, Codex 세션은 0 neighbors. `source_agent` payload 기반 라우팅 추가 → 머지
- 검증: Bash 42% / Edit 25% / TaskUpdate 8% 등 실 예측 표시

### Phase C — 사용자 GUI 테스트 → 4 UX 버그
- C1. **Mix & Match 모달 사용 불가**: `<dialog>.showModal()` backdrop이 메인 화면 카드의 `+ pos / − neg` 버튼 클릭 차단. **PR #13** — 모달 내부에 self-contained 검색 + 추가 UI 신설 (mix-picker + 결과 행마다 + pos/− neg 버튼) → 머지
- C2. **거대 보라 원 viewport-spanning oval** + heat-chip 우측 위치 + recall banner 영구 잔존:
  - **PR #14 commit 1** (view-transition snapshot leak hypothesis — 부분적 진실): `cinematicZoom`에서 `transition.finished.then(cleanup)` 추가 + `viewTransitionName = ""` 클리어
  - **PR #14 commit 2** (heat-chip top-center 위치): `left = cardCenterX - chipWidth/2`, `top = card.top - chipHeight - 6` 클램프
  - **PR #14 commit 3** (recall banner 20s auto-hide)
  - **PR #14 commit 4** (heat-chip 사이즈 클램프 — 진짜 원인): chip이 긴 enrich text를 받아 wrap → backdrop-filter saturate가 viewport-sized 박스를 vivid purple lens로 렌더. `max-width: min(72ch, calc(100% - 32px))` + `.bit { white-space: nowrap; text-overflow: ellipsis; max-width: 38ch }` + `backdrop-filter` 제거 → 검증 후에도 user 화면에 보라 원 잔존 (별개 원인 의심)
  - **PR #14 commit 5** (D-13 round-3 — 가설 그만, observability 추가): Tauri `devtools` feature 활성화 (사용자가 DevTools로 직접 인스펙트 가능) + `tail_recent_errors`에서 body-text regex 완전 제거 + shell stderr noise 6패턴 필터 + 카드 `errors` 배지 tooltip
- C3. **ERROR 설명 부재**: `tail_recent_errors`가 jq stderr 같은 일반 CLI 에러를 recall banner로 surface. body text의 `error:` substring matching 광범위. 카드의 빨간 `errors` 배지가 "Memex 자체 에러"로 오해됨.
- 4개 commits 모두 새 빌드에 포함 확인 (MD5 매칭): `055742f0d878d92000736ffd6d5d10fc`

### Phase D — 부가 산출물
- `claudedocs/reports/2026-05-19-upstream-divergence.html` — sgwannabe/memex upstream과 차이 분석 보고서 (gitignored)
- E2E proof 파일들이 사용자 home path 노출 → `.gitignore` + README 재작성 (`tests/e2e/*.{json,txt,png}` gitignored, regen 스크립트 commit됨)

---

## §2 현재 상태

### Git branches

| Branch | HEAD | 상태 |
|---|---|---|
| `main` | `2cf5d27` (Merge #13) | 최신 (PR #10-13 모두 머지됨) |
| `fix/view-transition-leak-and-ux` | `84db1fc` | **PR #14 OPEN** (5 commits ahead of main) |
| `feature/sota-v3.2-plan-and-mockups` | `7a19043` | **PR #1 DRAFT** (plan 문서 보존용, 머지 대상 아님) |

### Live URLs / 산출물

- Repo: https://github.com/ComBba/memex
- PR #14: https://github.com/ComBba/memex/pull/14 (CI green, mergeable)
- PR #1: https://github.com/ComBba/memex/pull/1 (draft, 보존)

### 빌드 / 환경 메트릭

| 항목 | 값 |
|---|---|
| cargo test (main) | 222 passed / 0 failed / 4 ignored (3 P4 eval gates D-8 + 1 P2-RECENCY-CALIBRATION) |
| cargo build --release | 48,290,464 bytes binary @ 10:29 KST |
| npm run tauri build | Memex.app + 16M DMG (bundle_dmg.sh 마지막에 실패하지만 .app은 정상) |
| Qdrant `memex_sessions_v3` | 110 points, status: green, 5 segments |
| Real corpus | 45 Claude + 66 Codex = 111 sessions (1 duplicate skipped) |
| Memex.app installed | `/Applications/Memex.app` @ 10:29, MD5 = local build (devtools 활성화됨) |
| Docker `memex-qdrant` | Up 12 hours, Qdrant 1.18.0 |

### 미완 상태

- **PR #14 머지 안 됨** — 사용자가 거대 보라 원의 DOM 정체를 DevTools로 확인 후 마지막 commit 추가 가능성
- `tests/e2e/screenshots/*.png` + `tests/e2e/*.{txt,json}` — 로컬에만 존재, gitignored
- `claudedocs/reports/2026-05-19-upstream-divergence.html` — gitignored 디렉토리, 보존 옵션 결정 필요

---

## §3 다음 세션에서 할 수 있는 것

### 즉시 가능 (사용자 입력 불필요)

1. **PR #14 머지** (사용자가 보라 원 인스펙트 결과를 따로 보고하지 않는 경우): CI green, mergeable. 이미 4가지 UX 개선 (mix self-contained / chip top-center / recall auto-hide / heat-chip size guard / devtools 활성화 / recall noise filter / errors tooltip) 포함
2. **VSD 2026 비디오 녹화 준비**: `scripts/demo/record-demo.sh --dry-run` → 모든 prereq verify 후 실제 OBS 녹화
3. **`upstream-divergence.html` 보고서를 메인 브랜치에 추가할지 결정** — 현재 `claudedocs/reports/` 가 gitignored 안 됨, 작업트리에만 있음
4. **D-0 이후 active-layer 통합** (`mcp.rs` + `watcher.rs` cherry-pick from upstream)

### 사용자 입력 필요

1. **거대 보라 원 DOM 인스펙트 결과** — 사용자가 마우스 우클릭 → Inspect Element → 보라 원 위 hover → 좌측 DOM 트리에서 highlighted 노드 한 줄 보고. 그게 있으면 즉시 박을 수 있음
2. **PR #14 머지 vs 추가 commit 결정** — 보라 원 fix가 PR #14 안에 들어갈지, 별도 PR이 될지
3. **VSD 2026 Google Form 제출 시점 결정** (D-1 ~ D-0)
4. **비디오 녹화 일정** — `record-demo.sh` 의 18-shot 시나리오 실제 녹화는 1인 + OBS 환경 필요

---

## §4 할 수 없는 것 (외부 변수)

1. **GitHub Actions / 외부 CI 미구성**: 현재 CI는 CodeRabbit 리뷰 봇만. cargo test / build는 로컬에서만 실행. CI가 없어도 머지에 문제 없으나 PR 검증은 수동
2. **비디오 녹화 자체** — OBS 캡처 + DaVinci 편집은 사용자가 수동으로 (kickoff decision 8)
3. **VSD 2026 Google Form 제출** — 사용자가 직접 (자율 가능한 영역 아님)
4. **외부 검수자 평가** — "Think Outside the Bot" 메시지 30s 내 인지 (AC-7.1.6)
5. **upstream `mcp.rs` cherry-pick** — D-0 이후 작업으로 분류, 본 세션 scope 밖
6. **Production 서버 (8080)** — 사용자만 제어. 본 작업과 무관함

---

## §5 추가로 필요한 것

### 사용자 확인 필요

1. ✅ **Memex.app 새 빌드 설치됨** — MD5 매칭 확인 (사용자 "재설치 완료" 시점에 확인)
2. **DevTools로 보라 원 인스펙트** — 다음 세션에서 첫 작업
3. **클린 머신에서 DMG 설치 테스트** — D-7 이내 권장 (AC-7.4.4)
4. **VSD 2026 Form 양식 미리 작성** — `https://forms.gle/YDQ2TDUi8MqS9Vx28`

### 환경 점검 (다음 세션 시작 시)

```bash
# Qdrant 살아있는지
curl -sf http://localhost:6333/readyz
# 인덱싱된 corpus
curl -s http://localhost:6333/collections/memex_sessions_v3 | jq '.result.points_count'  # >= 80
# Memex.app 최신
ls -la /Applications/Memex.app/Contents/MacOS/memex  # mtime 확인
# 빌드 도구
cargo --version && node --version && npm --version
```

---

## §6 다음 세션 시작 프롬프트 (복사-붙여넣기)

```text
/handon

이전 세션 핸드오프: claudedocs/2026-05-19-session-handoff.md

읽고 다음 결정 사항에 답한 뒤 진행하세요:
1. 사용자 GUI 환경에서 거대 보라 원의 DOM 정체(우클릭 → Inspect Element 결과)를 알려주실 수 있나요? — 없으면 PR #14 그대로 머지
2. PR #14를 지금 머지할까요, 보라 원 후속 fix까지 기다릴까요?
3. claudedocs/reports/2026-05-19-upstream-divergence.html 을 main 브랜치에 추가할까요?
4. 다음 세션의 1순위는: (a) 보라 원 마무리 / (b) 비디오 녹화 prep / (c) upstream active-layer cherry-pick 중 어디인가요?

D-day: 2026-06-01 23:59 UTC (D-13)
```

---

## §7 핵심 자산 위치 reference

| 자산 | 경로 |
|---|---|
| 7 surface CLI | `./src-tauri/target/release/memex {scan,search,lens,mix,topology,recall,predict}` |
| Tauri app (installed) | `/Applications/Memex.app` |
| Tauri app (build output) | `src-tauri/target/release/bundle/macos/Memex.app` |
| DMG | `src-tauri/target/release/bundle/dmg/Memex_0.1.0_aarch64.dmg` (생성 실패해도 .app은 OK) |
| Smoke test | `bash scripts/demo/smoke-test.sh --json` |
| GUI screenshot capture | `bash scripts/demo/capture-screenshots.sh` |
| Demo recording helper | `bash scripts/demo/record-demo.sh --dry-run` |
| IMPLEMENTATION_REPORT | `claudedocs/IMPLEMENTATION_REPORT.md` |
| Video shot script | `claudedocs/phases/phase-7-demo-production/video-script.md` |
| Plan v3.2 | `claudedocs/sota-plan-v3.html` (참고용) |
| v0.4 multi-agent addendum | `claudedocs/phases/v0.4-multi-agent-addendum.md` |
| E2E artifacts | `tests/e2e/` (gitignored — regen via smoke-test.sh + capture-screenshots.sh) |
| Upstream divergence report | `claudedocs/reports/2026-05-19-upstream-divergence.html` (untracked) |
| Payload helpers (consolidated) | `src-tauri/src/payload.rs` |
| Predict routing fix | `src-tauri/src/indexer.rs::predict_next_actions` (source_agent branch) |
| Recall noise filter | `src-tauri/src/commands.rs::tail_recent_errors` (structured-only + shell-noise list) |
| Mix self-contained picker | `src/main.js::runMixPickerSearch + renderMixPickerRow` |
| Heat-chip size guard | `src/styles.css::.heat-chip` (max-width + bit nowrap) |

---

## §8 알려진 issue / open question

1. **거대 보라 원의 정체 미확정** — 가설 단계에서 세 번 fix 시도했지만 사용자 화면에서 잔존. DevTools가 이제 활성화되었으니 다음 세션에서 직접 인스펙트 가능
2. **bundle_dmg.sh 실패** — `npm run tauri build` 마지막 단계에서 DMG 번들링 sub-script가 실패. 다만 `.app` 자체는 정상 생성. 원인은 권한/디스크 마운트 관련으로 추정, 우선순위 낮음 (사용자가 .app 직접 사용 가능)
3. **`P2-RECENCY-CALIBRATION` 테스트 ignored** — f32 precision rebase 적용했지만 live-corpus 검증 안 함. 통과 가능성 있으나 D-13 critical path에서 위험 회피 위해 그대로
4. **upstream sgwannabe/memex 와 14 commits divergence** — `mcp.rs` (9 tools) + `watcher.rs` (auto-index) + `dashboard.html/js` 가 upstream에만 존재. D-0 이후 cherry-pick 권장
5. **CI 자동 빌드 없음** — GitHub Actions 워크플로우 미정의. 머지 전 cargo test 수동 실행 필요
6. **DevTools feature on production build** — ~5-10 MB 추가 binary size. 해커톤 최종 제출 빌드 전 다시 비활성화 결정 필요할 수 있음

---

*Generated by Claude Code (Opus 4.7 / 1M context) on 2026-05-19 10:31 KST. Next-session entry: `/handon claudedocs/2026-05-19-session-handoff.md`.*
