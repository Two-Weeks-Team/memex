# Upstream PR Plan — ComBba/memex → sgwannabe/memex

**Generated**: 2026-05-19 (D-13, post-PR #15 merge)
**Fork HEAD**: `e28dbee` (Merge PR #15)
**Upstream HEAD**: `4973a91` (feat: data archaeology + dashboard + macOS Time Machine rail + watcher polish)
**Divergence**: fork +30 commits / upstream +13 commits

---

## §0 TL;DR

Fork은 hackathon scope(P1–P8 + WOW surfaces + UX polish)로 upstream과 크게 갈라져 있다. **단일 cherry-pick으로 upstream에 보낼 수 있는 commit은 없다** — 모든 candidate가 fork-only 모듈(`lens.rs`, `codex_parser.rs`, `heat-trail` JS/CSS) 또는 fork-only HTML 구조에 의존하기 때문이다.

세 가지 현실적 경로:
1. **Bug fix backport (low risk, small)** — UX bug fix를 upstream 환경에 맞춰 **다시 작성**해서 작은 PR로 보냄.
2. **Feature drop (high risk, big)** — fork의 P1–P8 전체를 단일 large PR로 묶어 upstream에 제안.
3. **유지 (no PR)** — Memex hackathon 제출 후 upstream과 갈라진 상태 유지.

권장: **D-0(2026-06-01) 이후** 1번 경로로 작은 PR 한두 개 시도하여 maintainer reaction을 본 뒤 결정.

---

## §1 환경 상태

| 항목 | 상태 |
|---|---|
| `origin` remote | `https://github.com/ComBba/memex.git` (fork) |
| `upstream` remote | `https://github.com/sgwannabe/memex.git` ✅ added 2026-05-19 |
| `main` (local) | clean, `origin/main`과 동기 |
| 머지된 feature/fix 브랜치 (local) | 모두 삭제 완료 (origin 보존) |
| 머지된 PR | #2–#15 (14개) |
| Open PR | 없음 (PR #1 DRAFT — plan 문서 보존용) |

---

## §2 Upstream PR 후보 분석

### 후보 A — `fix/predict-codex-routing` (PR #12 / commit `2b59dc9`)

**의도**: Codex 세션의 `predict_next_actions`가 Claude parser를 사용해서 0 neighbors를 반환하던 버그 fix.

**상태**: ❌ **단순 cherry-pick 불가**

**이유**:
- 의존: `src-tauri/src/codex_parser.rs` 모듈 — **upstream에 존재하지 않음** (fork P5에서 도입)
- 의존: `source_agent` payload field — fork-only schema (memex_sessions_v3, P3에서 도입)
- backport하려면 codex 파서 전체 + v3 schema marker가 동시에 가야 함 → 사실상 P3+P5 feature drop과 동격

**대안**: upstream의 `predict_next_actions`는 단일 parser 가정이므로 이 fix가 무의미.

---

### 후보 B — `fix/mix-modal-self-contained-picker` (PR #13 / commit `e402b1f`)

**의도**: Mix & Match `<dialog>`의 backdrop이 메인 카드의 `+ pos / − neg` 버튼을 막아서 modal이 무용지물이던 버그 fix. modal 내부에 self-contained 검색 + 추가 UI 신설.

**상태**: ⚠️ **수동 backport 필요 (3 파일 모두 conflict)**

**trial 결과** (2026-05-19):
```
git cherry-pick e402b1f → 자동 병합:
  src/index.html  → CONFLICT (mix-modal HTML 구조가 다름)
  src/main.js     → CONFLICT (모듈 import + state 객체 divergence)
  src/styles.css  → CONFLICT (~ 30 commits 누적 변경)
```

**backport 단계** (estimate: 1–2시간):
1. `git checkout -b backport/mix-modal-fix upstream/main`
2. fork의 e402b1f diff를 참고해서 **수동으로 다시 작성**:
   - `mix-picker` HTML 컨테이너 (index.html ~35 라인)
   - `runMixPickerSearch` + `renderMixPickerRow` JS (main.js ~159 라인)
   - `.mix-picker*` CSS (styles.css ~127 라인)
3. upstream `mix-modal` opening/closing flow와 통합
4. 검증: upstream main 빌드 + 수동 modal 테스트
5. PR open: `gh pr create --repo sgwannabe/memex --base main`

**가치**: ✅ 작고 명확한 UX 개선. maintainer가 받을 가능성 높음.

---

### 후보 C — `fix/heat-trail-purple-oval` (PR #15 / commit `deed283`)

**상태**: ❌ **upstream에 heat-trail 자체가 없음**

WOW-1 "Time Machine Heat Trail"은 fork의 P6 feature. upstream에 `#heat-trail` SVG, `drawHeatTrail()`, `HEAT_COLOR_*` 상수, `.heat-trail` CSS 모두 없음. 보낼 곳이 없으므로 PR 불가.

(다만 코드 패턴 자체 — `vector-effect="non-scaling-stroke"` + score clamp + viewBox guard — 는 일반화된 SVG 안티패턴 fix로 upstream의 다른 SVG 코드에 적용 가능할 수 있음. 별도 분석 필요.)

---

### 후보 D — P1 security hardening (commit `f55d417`)

`KF-01 path sandbox + KF-02 snapshot sandbox + KF-03 signed envelope + KH-01 multi-agent`

**상태**: ❓ **부분 backport 가능성 있음**

`KF-01 path sandbox`만 분리하면 upstream에도 의미 있는 작은 security fix가 될 수 있음. 의존성 분석 필요.

---

## §3 Upstream → Fork (반대 방향, 핸드오프 §3-#4 참조)

D-0(2026-06-01) 이후 작업:

| Upstream commit | Description | Cherry-pick 가치 |
|---|---|---|
| `d58804e` | feat: MCP server + 9 tools | 🟢 매우 높음 — Memex as agent memory layer |
| `96ba2dd` | feat: background auto-index daemon | 🟢 매우 높음 — mtime-keyed incremental |
| `b0fb159` | feat: macOS notifications | 🟡 중간 — proactive recall alerts |
| `4973a91` | feat: data archaeology + dashboard + Time Machine rail | 🟡 중간 — 일부 watcher polish는 필요 |
| watcher fixes (5 commits) | osascript fallback, debounce, single-fire 등 | 🟡 중간 — `b0fb159` cherry-pick 시 함께 |

---

## §4 권장 시퀀스

```
[D-13 ~ D-0]
  ✓ heat-trail fix 머지 (PR #15) — fork-only 안정화
  ✓ 환경 정리 (이 문서)
  - 비디오 녹화 + VSD 2026 제출 (fork 단독 진행)

[D-0+]
  1. Upstream PR 1 후보 backport (mix-modal) — manual rewrite
  2. Upstream cherry-pick (mcp.rs + watcher.rs + 알림) — fork에 active layer 도입
  3. P3 schema v3 backport 가능성 평가 (큰 PR)

[Long-term]
  - 정기적으로 upstream 변경 모니터링
  - fork-only feature를 점진적으로 upstream에 제안
```

---

## §5 Useful commands

```bash
# Sync upstream
git fetch upstream

# Compare
git log upstream/main..main --oneline           # fork ahead
git log main..upstream/main --oneline           # upstream ahead
git diff upstream/main main -- src/main.js      # specific file divergence

# Backport workflow (B 후보 예시)
git checkout -b backport/mix-modal upstream/main
git show e402b1f -- src/index.html              # reference diff
# (manual rewrite)
git commit -s -m "fix(mix): self-contained Mix & Match picker"
git push origin backport/mix-modal
gh pr create --repo sgwannabe/memex --base main --title "..."
```

---

*Generated by Claude Code (Opus 4.7 / 1M context). See also: `claudedocs/2026-05-19-session-handoff.md` for prior context.*
