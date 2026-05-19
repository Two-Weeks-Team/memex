# Upstream PR Submission Package — `ComBba/memex` → `sgwannabe/memex`

**Generated**: 2026-05-19 (D-13, post-PR #15 merge)
**Method**: 7-agent parallel analysis (system-architect, frontend-architect, backend-architect × 2, security-engineer, quality-engineer, technical-writer, devops-architect, review-analyzer)

---

## TL;DR — what to submit, in priority order

| # | PR | Branch | Verdict | Effort |
|---|---|---|---|---|
| **1** | mix-modal self-contained picker | `upstream-pr/mix-modal-picker` | ✅ SHIP | manual rewrite, ~250 LOC, 0 Rust |
| **2** | recall stderr filter + errors badge tooltip | `upstream-pr/recall-filter-and-errors-tooltip` | ✅ SHIP (after slicing `84db1fc`) | near-direct, ~50 LOC, 3 files |
| **3** | KF-01 path sandbox (security) | `upstream-pr/kf01-path-sandbox` | ⚠️ SHIP after maintainer interest-check issue | ~330 LOC, +`dirs` dep |

**Do NOT submit**:
- `deed283` heat-trail purple oval fix — feature doesn't exist upstream
- `e1c075b` cli ensure v3 — bug doesn't exist upstream (fork-only v3 schema)
- `2b59dc9` predict Codex parser — depends on fork-only `codex_parser` module
- WebView devtools enable from `84db1fc` — **BLOCKED** by security review (IPC exfiltration vector if released in production builds)

---

## Document map

```
claudedocs/upstream-pr/
├── README.md                            ← you are here
├── 00-divergence-matrix.md              ← system-architect: file-level map + per-commit verdict
├── candidates/
│   ├── 01-mix-modal-backport.md         ← frontend-architect: exact 3-file patch
│   └── 02-p1-security-backport.md       ← backend-architect: KF-01 surgical extraction
├── reviews/
│   ├── security-review.md               ← security-engineer: OWASP + per-commit verdict
│   ├── test-plan.md                     ← quality-engineer: unit/integration/property/manual
│   ├── cicd-impact.md                   ← devops-architect: build/CI/release impact
│   └── maintainer-q-and-a.md            ← review-analyzer: predicted reviewer questions
└── artifacts/
    ├── README.md                        ← how to use the body files
    ├── pr-descriptions.md               ← all PR bodies in one file
    ├── pr-A-body.md                     ← mix-modal (extracted for --body-file)
    ├── pr-B-body.md                     ← KF-01 path sandbox
    └── pr-recall-body.md                ← recall filter + errors tooltip
```

---

## Cross-cutting findings from the 7-agent panel

### 1. The published divergence numbers were off
`UPSTREAM_PR_PLAN.md` said fork +30 / upstream +13. Actual: **+35 / +14**. Counts were taken before later doc/hotfix commits landed.

### 2. Schema reality is dual, not "fork-only v3"
Fork keeps `COLLECTION = "memex_sessions"` (v2) for read-fallback while writes target `COLLECTION_V3`. The 3 backport candidates above do not touch payload, so **no schema migration is needed**.

### 3. Upstream gained `parse_transcript_session` (+283 LOC) that fork lacks
Any fork→upstream PR that compiles `parser.rs` will see those 283 lines on the diff and conflict. None of our 3 ship candidates touch `parser.rs`.

### 4. WebView devtools in production is a security blocker
`84db1fc` was committed with `devtools` Tauri feature enabled unconditionally. In release builds this lets an attacker with physical access call any `window.__TAURI__.invoke()` from the console, bypassing the capability layer. **All three PRs above explicitly exclude this change.** If we want devtools in dev builds, gate it behind a Cargo feature: `[features] devtools = ["tauri/devtools"]`.

### 5. PR red flags an experienced reviewer will catch
Strip from every upstream PR (per Q&A appendix):
- Internal phase jargon: `// P3 KG-03`, `// KC-01b`, `// KF-01`, `// WOW-3`
- `Co-Authored-By: Claude Opus 4.7 (1M context)` trailer
- `🤖 Generated with Claude Code` footer
- Deferred audit item references (`MED-2`, `LOW-1`, `NIT-1`)
- `"dev.sgwannabe.memex"` hardcoded in `snapshot.rs:46` — derive via Tauri API
- `qdrant_version: "1.18.0"` constant — add comment "minimum tested version, not a pin"

### 6. Upstream has zero CI today
No GitHub Actions workflows, no Dockerfile, no docker-compose.yml. Acceptance is manual `cargo build --release` on macOS aarch64. We are not blocked by this, but we should:
- Make verification commands explicit in every PR body
- Optionally propose a starter `build.yml` in a separate "infra" PR

---

## Submission workflow

For each PR (in priority order):

```bash
# 1. Final sanity check
git fetch upstream && git checkout upstream-pr/<branch> && git rebase upstream/main

# 2. Verify the branch still builds
cd src-tauri && cargo check && cargo test 2>&1 | tail -20
cd .. && npm install --silent && npm run build 2>&1 | tail -10

# 3. Push (already on origin from agent step)
git push origin HEAD

# 4. Open PR using extracted body file
gh pr create \
  --repo sgwannabe/memex \
  --base main \
  --head ComBba:upstream-pr/<branch> \
  --title "$(head -1 claudedocs/upstream-pr/artifacts/pr-<X>-body.md | sed 's/^# //')" \
  --body-file claudedocs/upstream-pr/artifacts/pr-<X>-body.md

# 5. After opening, paste any screenshots from claudedocs/reports/ into the PR comments
```

---

## Per-PR submission checklist

Use before opening each PR:

- [ ] Branch is `upstream-pr/<purpose>`, not a `feature/p*` or `hotfix/*`
- [ ] Branched from `upstream/main`, not `origin/main` (verify with `git log --oneline upstream/main..HEAD` shows ONLY this PR's commits)
- [ ] Commit message has no `Co-Authored-By: Claude` trailer
- [ ] Commit message has no `🤖 Generated with Claude Code` footer
- [ ] No internal phase markers (`KF-01`, `P3`, `WOW-1`, etc.) in code comments
- [ ] No hardcoded fork bundle id (`dev.sgwannabe.memex`)
- [ ] `cargo check` clean
- [ ] `cargo test` — own tests pass (unrelated pre-existing failures acceptable)
- [ ] `npm run build` passes (if PR touches frontend)
- [ ] Screenshots captured for UX PRs (mix-modal: before+after of dialog flow)
- [ ] PR body uses the artifact file via `--body-file`
- [ ] Title is conventional commit style, ≤70 chars
- [ ] Reviewer suggestions noted in PR body or as request-review on the PR

---

## What this package does NOT include

- **Actual PR submission to upstream** — that requires your decision + GitHub auth + maintainer relationship. We staged everything; you open.
- **Pre-PR maintainer interest-check issue for KF-01** — security PRs benefit from a small issue first ("would you accept a path-sandbox patch?"). Draft included in `artifacts/pr-B-body.md` but you decide if you file the issue first or jump straight to PR.
- **The hackathon-attribution decision** — should we mention this came from a hackathon fork? Q&A recommends "yes in the PR description body, no in commit messages". Already reflected in artifacts.

---

## Next session

If you decide not to submit immediately (e.g., focus on D-day video first), the branches stay on `origin/upstream-pr/*` and `upstream/main` won't move much in 13 days. Re-run the verification commands before submitting and you're good.

If you want to submit *now*:
1. Pick PR #1 (mix-modal) — lowest risk, highest UX value
2. Run the 5-step submission workflow above
3. Wait 24-48h for maintainer reaction
4. Use feedback to calibrate PR #2 / #3 tone

---

*Generated by Claude Code (Opus 4.7 / 1M context) via 7-agent parallel analysis on 2026-05-19. See individual agent reports for full reasoning.*
