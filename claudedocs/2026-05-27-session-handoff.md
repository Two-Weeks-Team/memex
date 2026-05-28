# Session Handoff — 2026-05-27 (D-5)

**Repo**: https://github.com/Two-Weeks-Team/memex (org canonical · `origin`, no fork)
**Branch at session end**: `feat/web-service-stack`
**HEAD commit**: `f47c2ce` (docs(web): make the reproduce commands actually runnable)
**D-day**: 2026-06-01 23:59 (VSD 2026 submission)

---

## §0 두 줄 요약

이 세션은 **서버 변형(단일 Docker 이미지 = Qdrant + web UI + JSON API + MCP)** 을 PR #6에 "수상용 완성본"으로 마감했다 — 전 표면 브라우저 E2E + 오프라인 컨테이너 + MCP 2종 전송 검증, web-vs-app 근거 문서, 서버 변형을 소개하는 랜딩 섹션 완성, 그리고 사용자 품질 지적에 따라 **브라우저에서 죽어있던 Dashboard 결함을 발견·수정**(셰임 미주입 + 미구현 명령 2개)했다. **다음 세션 1순위**: PR #6 머지 여부 결정(`gh pr merge 6 --merge`) + 팀원 PR #7(Companion/Wrapped/Loop Breaker)과의 통합 순서 정리.

---

## §1 진행한 작업 (시간순)

직전 세션(`2026-05-19`) 이후 별도 세션에서 PR #4(judge-path 하드닝), PR #5(dep upgrade)가 머지됐고, 이번 세션은 PR #6(서버 변형)을 두 패스로 마감.

### Phase A — `/goal` 완성 패스 (10 DONE 항목)
`/goal-start` → `/goal`(16h cap, "수상 지향 완성본 + 시각 WOW + 랜딩 완성")로 실행:
- **불가능 항목 선언**(I1–I5): Windows/Intel HW 없음, 앱 notarization 불가(adhoc-signed), 네이티브 amd64 불가(arm64 호스트), 공개 Pages 렌더 불가, 멀티머신 원격 MCP 불가 — 결과 전에 정직하게 명시
- **백엔드 E2E**: gui build exit 0 · web build(`--no-default-features --features web`) exit 0 · **228 tests pass** · CI green
- **컨테이너 오프라인 E2E**: `docker run --network none` → 비루트 `memex`, PID1 `tini`, 외부 DNS 차단, auto-index 12/12, 전 JSON 표면 실데이터
- **MCP 2종 전송**: HTTP `/mcp` 9 tools + `tools/call`; stdio `memex mcp` 9 tools + `tools/call`
- **브라우저 E2E**(Playwright): Time Machine·search·topology·predict 구동, 콘솔 클린
- **commit `4932ceb`** fix(web): plugin:* IPC → null + favicon 주입 (콘솔 404 2건 제거)
- **commit `11d68b1`** docs(web): `docs/web-vs-app.md` 8축 비교 매트릭스 + 정직한 "앱이 이기는 영역" + verdict
- **commit `cdceb16`** feat(landing): "Run it your way" 섹션(앱 vs Docker 단일 이미지) + 브라우저 UI 스크린샷

### Phase B — 사용자 품질 지적 → 심층 감사 + 실제 결함 수정
사용자: "랜딩과 모든 것들이 실제로 완성된게 맞아?" + ":8080/.env 제약 해제".
가정 대신 **전 표면을 실제로 구동**해 결함 발견:
- **Dashboard가 브라우저에서 죽어 있었음**: `__TAURI__` 셰임이 `index.html`에만 주입돼 `dashboard.html`이 `undefined (reading 'core')`로 데이터 0 로드
- UI가 호출하는 `prompt_history_stats`·`snapshot_export` 2개가 web 디스패처 **미구현**(404)
- **commit `17f728a`** fix(web): 셰임을 `html_with_shim()`로 일반화 → `/index.html`+`/dashboard.html` 모두 라우팅; 두 명령 구현. web 디스패처가 이제 UI의 **16개 명령 100%** 처리
- **commit `1e8f6c6`** docs(web): 컨테이너 Dashboard 스크린샷 + web-vs-app에 4표면 갤러리
- **commit `f47c2ce`** docs(web): web-vs-app §A 재현 명령이 이미지에 없는 `file` 사용 → magic-byte 검사로 교체
- 재검증(컨테이너, 캐시 우회): Dashboard 채워짐 + **콘솔 에러 0**; Replay·Mix & Match도 브라우저 구동 확인
- 메모리 기록: `:8080`/`.env` 제약 해제 + "시간 무제한=품질 기대치" → `memex_constraints_lifted.md`

---

## §2 현재 상태

### Git branches
| Branch | 상태 | 비고 |
|---|---|---|
| `feat/web-service-stack` | **이번 작업 · HEAD `f47c2ce`** | PR #6, 12 commits, **미머지** |
| `main` | base | PR #4·#5 머지 반영 |
| `sgwannabe/qdrant-hackathon-analysis` | 팀원 PR #7 | Companion/Wrapped/Loop Breaker (병렬) |

### Open PRs
| PR | 제목 | 상태 |
|---|---|---|
| **#6** | server variant — single Docker image (Qdrant + web + MCP) | **OPEN · MERGEABLE · CI green @ `f47c2ce`** |
| #7 | Companion + Wrapped + Loop Breaker — agent memory layer | OPEN (팀원 sgwannabe) |

### 빌드/검증 메트릭 (이 세션 실측)
| 항목 | 값 |
|---|---|
| Rust 테스트 | 228 passed / 0 failed / 4 ignored (single-thread, Qdrant up) |
| gui release build | exit 0 (Mach-O arm64) |
| web build (`--features web`) | exit 0 (in-image: ELF Linux) |
| Docker 이미지 | `memex-allinone:latest` **556 MB** |
| 오프라인 컨테이너 | `--network none` OK · 비루트 · tini PID1 · auto-index 12/12 |
| MCP tools | HTTP 9 · stdio 9 |
| 브라우저 표면(콘솔 에러 0) | Time Machine·Search·Lens·Replay·Mix&Match·Predict·Topology·Dashboard |
| 랜딩 | 8 섹션 렌더 · 이미지 5/5 · 깨진 링크 0 |
| CI (PR #6 @ f47c2ce) | Rust 2m56s pass · Frontend pass · CodeRabbit pass |

### 환경
cargo 1.93.0 · docker 29.3.1 · node v24.15.0 · python 3.12.4 · `memex-qdrant` 컨테이너 가동 중(:6333/:6334, 사용자 소유 — 건드리지 말 것)

---

## §3 다음 세션에서 할 수 있는 것

### 즉시 가능 (사용자 입력 불필요)
- 로컬 재현: `docker run --rm -p 8765:8765 memex-allinone` → http://localhost:8765 (UI) · `/dashboard.html` · `/mcp`
- 컨테이너 재검증: `deploy/web/README.md` 또는 `docs/web-vs-app.md` "Reproduce" 블록
- 랜딩 미리보기: repo root에서 `python3 -m http.server 8088` → http://localhost:8088/index.html
- web 디스패처 명령 커버리지 재확인: `src-tauri/src/web.rs` `dispatch_invoke` (16/16)

### 사용자 입력 필요 (→ §6 결정 사항)
- PR #6 머지 시점/방식
- PR #7과의 통합 순서 (둘 다 main 대상 · 충돌 가능성 점검 필요)
- 멀티아키(amd64) 이미지 빌드 + 레지스트리 푸시 여부
- GitHub Pages 실제 배포 여부

---

## §4 할 수 없는 것 (외부 변수)

- **PR #6 머지** — `--merge`만 허용(squash 금지). 이번 세션 정책상 머지하지 않음. 사용자 또는 리뷰어 승인 필요.
- **실 Windows/Intel-macOS HW 검증** — arm64 단일 호스트. 웹 변형은 Linux 컨테이너로 "어디서나" 증명, 앱은 macOS 전용.
- **앱 notarization** — Apple Developer cert 없음 → adhoc-signed.
- **네이티브 amd64 이미지** — arm64 호스트 → QEMU 에뮬레이션만. `docker buildx` 멀티아키는 문서화됐으나 벤치 안 함.
- **GitHub Pages 공개 렌더** — `sgwannabe.github.io` 푸시 권한/배포는 외부. 로컬 HTTP로만 검증.
- **PR #7 내용** — 팀원(sgwannabe) 병렬 작업. 통합 영향 미검토.

---

## §5 추가로 필요한 것

- **PR #6 ↔ PR #7 충돌 점검**: 두 PR 모두 main 대상. 머지 순서에 따라 rebase 필요할 수 있음. (`src/main.js`, `src-tauri/src/lib.rs` 등 공통 파일 충돌 가능)
- **환경 점검 (다음 세션 시작 시)**: `memex-qdrant` 컨테이너 가동 여부(`docker ps`), 이미지 존재(`docker images | grep memex-allinone`), Qdrant readiness(`curl -fsS localhost:6333/readyz`)
- **이미지 재빌드 트리거**: `src-tauri/src/web.rs` 또는 `src/**` 변경 시 `docker build -t memex-allinone -f deploy/web/Dockerfile .` 필요(이미지에 코드/UI 베이크됨)

---

## §6 다음 세션 시작 프롬프트

```text
/handon

이전 세션 핸드오프: claudedocs/2026-05-27-session-handoff.md

읽고 다음 결정 사항에 답한 뒤 진행하세요:
1. PR #6 (server variant)를 지금 머지할까요? (gh pr merge 6 --merge) 아니면 추가 작업/리뷰 대기?
2. PR #7 (팀원 Companion/Wrapped/Loop Breaker)과의 통합 순서는? (#6 먼저 / #7 먼저 / 충돌 점검 후 결정)
3. amd64 멀티아키 이미지 빌드 + 레지스트리(ghcr 등) 푸시를 진행할까요?
4. 랜딩페이지를 GitHub Pages에 실제 배포할까요? (sgwannabe.github.io 권한 확인 필요)

제약: :8080/.env 제약 해제됨(memex 한정) · 병합은 --merge만(squash 금지) · 외부 작업은 사전 보고
D-day: 2026-06-01 (D-5)
```

---

## §7 핵심 자산 위치 reference

| 자산 | 경로 |
|---|---|
| web 서버 (axum: UI/API/MCP/serve) | `src-tauri/src/web.rs` |
| web/gui feature 게이트 | `src-tauri/Cargo.toml` · `src-tauri/build.rs` · `src-tauri/src/lib.rs` · `main.rs` |
| 재사용 MCP 핸들러 | `src-tauri/src/mcp.rs` (`handle_rpc_value`, `new_shared_state`) |
| Tauri-free SessionSummary | `src-tauri/src/summary.rs` |
| 단일 이미지 빌드 | `deploy/web/Dockerfile` · `deploy/web/entrypoint.sh` · `.dockerignore` |
| 서버 변형 실행 가이드 | `deploy/web/README.md` |
| web-vs-app 근거 문서 | `docs/web-vs-app.md` |
| 브라우저 UI 스크린샷 | `docs/img/web/{web-timemachine,web-search,web-topology,web-dashboard}.png` |
| 랜딩 페이지 | `index.html` ("Run it your way" 섹션) |
| 프런트엔드 (셰임 대상) | `src/index.html` · `src/dashboard.html` · `src/main.js` · `src/dashboard.js` |
| 샘플 corpus (합성) | `examples/sample-corpus/` (12 sessions) |
| 코어 E2E 증거 | `docs/e2e-evidence.md` |

---

## §8 알려진 issue / open question

- **`tail_recent_errors`는 서버 변형에서 항상 빈 배열** — 정적 서버 corpus엔 변하는 라이브 세션이 없음(설계상 정상, stub 아님). `web.rs`에 명시적 주석.
- **Dashboard의 prompt-history 셀(PROMPTS PRESERVED/CORPUS SPAN/DISTINCT PROJECTS)은 컨테이너에서 빈 값** — 서버엔 `~/.claude/history.jsonl`이 없어 `prompt_history_stats`가 graceful empty 반환(정상). 세션 기반 셀(SESSIONS/TURNS/TOOL CALLS/프로젝트 분포/Pareto)은 채워짐.
- **이미지는 arm64 전용** — 멀티아키 미빌드. 다른 아키 호스트에서 실행하려면 buildx 필요.
- **CI는 web feature를 빌드하지 않음** — gui 타깃만 Linux 빌드. web 빌드는 로컬+Docker로만 검증됨. (CI에 web build/nestia-staleness 잡 추가는 선택지)
- **PR #6 ↔ #7 통합 미검토** — 공통 파일 충돌 가능성.
- **claudedocs/ 미추적 파일**(`2026-05-19-session-handoff.md`, `reports/`)은 이전부터 untracked — 이번 작업과 무관, 커밋하지 않음.
