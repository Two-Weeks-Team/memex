# Session Handoff — 2026-05-27 (D-5) · part 2

**Repo**: https://github.com/Two-Weeks-Team/memex (org canonical · `origin`, no fork)
**Branch at session end**: `feat/web-service-stack` (unchanged; HEAD `f47c2ce`)
**D-day**: 2026-06-01 23:59 (VSD 2026 submission)
**Prior handoff this day**: `claudedocs/2026-05-27-session-handoff.md` (part 1) — read that first for the PR #6 server-variant context.

---

## §0 두 줄 요약

이 세션은 part-1 핸드오프를 `/handon`으로 로드해 **4개 결정에 답한 뒤**, 그중 두 가지를 실행했다 — **랜딩 페이지를 GitHub Pages에 실제 배포**(org repo `gh-pages` 브랜치, https://two-weeks-team.github.io/memex/ 라이브·검증 완료)하고 **amd64 멀티아키 이미지를 buildx+QEMU로 빌드**(푸시 안 함, 성공). PR #6·#7은 사용자 결정에 따라 **건드리지 않았다**. 추가로 사용자가 `/statusline`(P10k 스타일 커스텀)과 `/claude-hud:setup`(플러그인)을 연달아 실행 → **statusline은 최종적으로 claude-hud로 설정됨**. **다음 세션 1순위**: PR #6 머지 여부 재결정(아직 미머지) + PR #7 통합 순서.

---

## §1 진행한 작업 (시간순)

### Phase A — `/handon`: part-1 핸드오프 로드 + 4결정 응답
part-1 핸드오프(`2026-05-27-session-handoff.md`)를 로드. git/PR 상태가 핸드오프 시점과 동일함을 확인(HEAD `f47c2ce`, PR #6/#7 OPEN, 변동 0). AskUserQuestion으로 §6의 4개 결정 수집:
1. **PR #6** → "추가 작업/리뷰 대기" (머지 안 함)
2. **PR #7** → "지금은 보류" (통합 안 함)
3. **amd64 멀티아키** → "빌드만 (푸시 안 함)"
4. **GitHub Pages** → "배포 진행"

### Phase B — GitHub Pages 배포 (실행 + 검증)
- 사전 점검: org repo는 **public**(무료 플랜 Pages 가능), 나는 **admin** 권한 보유. Pages 미설정 상태(404). 랜딩(`index.html`)은 `main`엔 **구버전**, 최신은 `feat/web-service-stack`에만 존재(PR #6 미머지).
- 링크 분석: `index.html`이 필요로 하는 로컬 자산은 `docs/img/*.png` 5개뿐, 나머지는 모두 절대 `https://` 링크.
- **PR #6를 머지하지 않고** 현재(최신) 랜딩을 배포하기 위해 **orphan `gh-pages` 브랜치** 생성:
  - 내용 = `index.html` + `docs/img/`(11 이미지) + `.nojekyll`, `feat/web-service-stack` 워킹트리에서 복사
  - 커밋 `f3046fc` → `git push --force origin gh-pages`
  - Pages 활성화: `gh api -X POST .../pages` source=`gh-pages` / root
- **검증(라이브)**: build `built`(22s, no error) · index `200` + 올바른 `<title>` + 4섹션 · 이미지 5/5 `200` · `.nojekyll` `200`
- 메모리 기록: `memex_pages_deploy.md` (라이브 URL + gh-pages 메커니즘 + 미머지 배포 이유)

### Phase C — amd64 멀티아키 이미지 빌드 (푸시 없음)
- `docker buildx build --platform linux/amd64 -t memex-allinone:amd64 -f deploy/web/Dockerfile --load .` (백그라운드, QEMU 에뮬레이션)
- Dockerfile은 builder 스테이지에서 Rust를 **컴파일**하고 runtime에서 `memex warm-embedder`로 모델을 베이크 → 에뮬레이션 하 콜드 빌드. **exit 0**으로 완료.
- **검증**: 이미지 `arch=amd64 os=linux`, content 165MB(on-disk 543MB) · 바이너리 `x86_64` ELF(magic `7f 45 4c 46`) · 에뮬레이션으로 `memex --version` 실행 exit 0. → part-1 §4의 "벤치 안 된 amd64 buildx 경로"가 실제 동작함을 확인. **레지스트리 푸시는 안 함**(결정대로).

### Phase D — statusline (사용자 커맨드 2건)
- `/statusline`: statusline-setup 에이전트가 `~/.p10k.zsh` 요소를 추출해 **P10k 스타일 커스텀** `~/.claude/statusline-command.sh` 생성 + `settings.json` 연결. 렌더 검증 중 `\~` 백슬래시 버그 발견·수정, ctx% green→yellow(>75%)→red(>90%) 동작 확인.
- `/claude-hud:setup`: 사용자가 claude-hud 플러그인 설치 후 셋업 실행 → statusLine을 **claude-hud로 교체**(bun 1.3.9 → `src/index.ts`). 커맨드 테스트 3줄 출력 exit 0. 옵션 기능 활성화: Tools activity + Agents & Todos + Session info → `~/.claude/plugins/claude-hud/config.json`.
- **순효과**: 현재 statusLine = **claude-hud**. P10k 커스텀 스크립트(`~/.claude/statusline-command.sh`)는 **orphan**(남겨둠, 되돌리기용). **claude-hud는 재시작 후 표시됨**.

---

## §2 현재 상태

### Git branches
| Branch | 상태 | 비고 |
|---|---|---|
| `feat/web-service-stack` | HEAD `f47c2ce` · **이번 세션 commit 0** | PR #6, 미머지 (변동 없음) |
| `gh-pages` | **신규 · `f3046fc`** | Pages 소스 (orphan, 정적 랜딩만) |
| `main` | base | PR #4·#5 반영, 랜딩은 구버전 |
| `sgwannabe/qdrant-hackathon-analysis` | 팀원 PR #7 | Companion/Wrapped/Loop Breaker |

### Open PRs
| PR | 제목 | 상태 |
|---|---|---|
| #6 | server variant — single Docker image (Qdrant+web+MCP) | OPEN · MERGEABLE · CLEAN (미머지) |
| #7 | Companion + Wrapped + Loop Breaker — agent memory layer | OPEN · MERGEABLE · CLEAN (팀원) |

### Live URLs
| 자산 | URL | 상태 |
|---|---|---|
| **랜딩 (GitHub Pages)** | https://two-weeks-team.github.io/memex/ | **라이브 · index 200 · built** |

### 빌드/검증 메트릭 (이 세션 실측)
| 항목 | 값 |
|---|---|
| Pages build | `built` 22s, error 없음, commit `f3046fc` |
| Pages 자산 | index `200` · 이미지 5/5 `200` · `.nojekyll` `200` |
| amd64 이미지 | `memex-allinone:amd64` · arch=amd64/linux · content 165MB / on-disk 543MB · exit 0 |
| amd64 바이너리 | `x86_64` ELF, 에뮬레이션 실행 OK |
| statusLine | claude-hud (테스트 3줄 출력 exit 0) |

### 환경
node v24.15.0 · python 3.12.4 · docker 29.3.1 · bun 1.3.9 · cargo 1.93.0
`memex-qdrant` 컨테이너(:6333/:6334) 사용자 소유 — 건드리지 말 것.

---

## §3 다음 세션에서 할 수 있는 것

### 즉시 가능 (사용자 입력 불필요)
- Pages 라이브 확인: `curl -I https://two-weeks-team.github.io/memex/`
- Pages 재배포(랜딩 변경 시): `index.html`+`docs/img`+`.nojekyll`을 temp dir에 모아 commit → `git push --force origin gh-pages` (Pages 자동 리빌드). 상세는 메모리 `memex_pages_deploy.md`.
- amd64 이미지 재실행: `docker run --rm -p 8765:8765 memex-allinone:amd64` (에뮬레이션, 느림)
- PR #6 로컬 재현: `docker run --rm -p 8765:8765 memex-allinone:latest` → http://localhost:8765

### 사용자 입력 필요 (→ §6 결정 사항)
- PR #6 머지 시점/방식 (여전히 미결)
- PR #7 통합 순서 (여전히 보류 중)
- amd64 이미지 **레지스트리 푸시** 여부 (이번엔 빌드만 함)
- Pages를 org repo(`two-weeks-team.github.io/memex`, 현재)로 유지할지 vs 팀원 `sgwannabe.github.io`로 옮길지

---

## §4 할 수 없는 것 (외부 변수)

- **PR #6 머지** — `--merge`만 허용(squash 금지). 사용자가 "대기" 결정 → 미실행.
- **amd64 레지스트리 푸시** — 사용자가 "빌드만" 결정 → ghcr 등 푸시 안 함. (이미지는 로컬에만 존재)
- **`sgwannabe.github.io` 배포** — 팀원 user-pages 저장소, 나는 접근 권한 없음. 그래서 org repo Pages로 배포함.
- **PR #7 내용/통합** — 팀원(sgwannabe) 병렬 작업. 미검토.
- **네이티브 amd64 (비에뮬레이션)** — arm64 단일 호스트. amd64는 QEMU 에뮬레이션으로만 빌드/실행됨.

---

## §5 추가로 필요한 것

- **PR #6 ↔ #7 충돌 점검**: 둘 다 main 대상. 공통 파일(`src/main.js`, `src-tauri/src/lib.rs` 등) 충돌 가능. 머지 순서 결정 시 rebase 필요할 수 있음.
- **statusline 재시작**: claude-hud는 **Claude Code 재시작 후** 표시됨. 안 보이면 `/claude-hud:setup` 재실행으로 Step 5 검증.
- **환경 점검(다음 세션 시작 시)**: `docker ps | grep memex-qdrant`, `docker images | grep memex-allinone`(latest+amd64 둘 다 있어야), `curl -fsS localhost:6333/readyz`.
- **gh-pages 동기화**: `feat/web-service-stack`의 랜딩이 바뀌면 `gh-pages`는 자동 갱신 안 됨 — 수동 재푸시 필요(§3).

---

## §6 다음 세션 시작 프롬프트

```text
/handon

이전 세션 핸드오프: claudedocs/2026-05-27-session-handoff-2.md

읽고 다음 결정 사항에 답한 뒤 진행하세요:
1. PR #6 (server variant)를 지금 머지할까요? (gh pr merge 6 --merge) 아니면 계속 대기?
2. PR #7 (팀원 Companion/Wrapped/Loop Breaker)과의 통합 순서는? (#6 먼저 / #7 먼저 / 충돌 점검 후)
3. amd64 이미지를 레지스트리(ghcr 등)에 푸시할까요? (이번엔 빌드만 했음)
4. 랜딩 Pages를 org repo(two-weeks-team.github.io/memex, 현재 라이브)로 유지? 아니면 다른 도메인?

제약: :8080/.env 제약 해제됨(memex 한정) · 병합은 --merge만(squash 금지) · 외부 작업은 사전 보고
D-day: 2026-06-01 (D-5)
```

---

## §7 핵심 자산 위치 reference

| 자산 | 경로 |
|---|---|
| 랜딩 페이지 (Pages 소스) | `index.html` + `docs/img/` (gh-pages 브랜치 = 이들의 복사본) |
| Pages 배포 메모리 | `~/.claude/projects/-Users-kimsejun-Documents-GitHub-memex/memory/memex_pages_deploy.md` |
| 단일 이미지 빌드 | `deploy/web/Dockerfile` · `deploy/web/entrypoint.sh` |
| amd64 빌드 명령 | `docker buildx build --platform linux/amd64 -t memex-allinone:amd64 -f deploy/web/Dockerfile --load .` |
| web 서버 (axum) | `src-tauri/src/web.rs` |
| web-vs-app 근거 | `docs/web-vs-app.md` |
| statusline (claude-hud) | `~/.claude/settings.json` statusLine + `~/.claude/plugins/claude-hud/config.json` |
| statusline (P10k, orphan) | `~/.claude/statusline-command.sh` (미사용, 되돌리기용) |
| part-1 핸드오프 | `claudedocs/2026-05-27-session-handoff.md` |

---

## §8 알려진 issue / open question

- **PR #6 여전히 미머지** — 사용자 결정 대기. CI green·CLEAN 상태 유지.
- **gh-pages는 수동 동기화** — 랜딩 변경 시 자동 반영 안 됨. PR #6 머지 후에도 main 기준 자동 Pages 워크플로(actions/deploy-pages)는 별도 구성 필요(현재는 브랜치 소스 방식).
- **amd64 이미지는 로컬에만** — 푸시 안 함. 다른 머신에서 쓰려면 레지스트리 푸시 필요.
- **statusline 두 개 공존** — claude-hud(활성) + P10k 스크립트(orphan). 재시작 전까지 현재 세션엔 미반영.
- **PR #6 ↔ #7 통합 미검토** — 공통 파일 충돌 가능성.
- **claudedocs/ 미추적 파일** (`2026-05-19`, `2026-05-27`(part1·2), `reports/`) — 이전부터 untracked, 이번 작업과 무관, 커밋 안 함.
