# P8 — E2E Validation Evidence

This directory is the home for **empirical proof artifacts** produced by
the P8 helper scripts when validating the 7 Memex surfaces against a live
local Qdrant 1.18 instance and the user's own session corpus.

## ⚠️ Why these files are NOT committed

The generated outputs contain **real private data** from whichever
machine the scripts were run on:

- session UUIDs and timestamps from `~/.claude/projects` and
  `~/.codex/sessions`
- user home paths (`/Users/<name>/.claude/...`, `/Users/<name>/.codex/...`)
- private project names and `ai_title`s extracted from session
  conversations
- UI screenshots showing the same private data rendered in the Memex
  window (Time Machine stack, Predict panel, Topology session list, etc.)

Committing this dataset would publish the contributor's workspace to
everyone who clones the repo. Codex flagged this as a **P1 privacy
issue** during external review (PR #10, `tests/e2e/topology.json:22`).
The directory is now `.gitignore`d for everything except this README
and the scripts that regenerate the artifacts — reviewers who want
to verify the surfaces actually work simply re-run the two helpers
below against their own corpus.

## How to regenerate the evidence

```bash
# 1) Start Qdrant (one-time, persists across runs)
docker run -d --name memex-qdrant -p 6333:6333 -p 6334:6334 \
  qdrant/qdrant:v1.18.0

# 2) Index the real local corpus (Claude + Codex)
./src-tauri/target/release/memex scan --index
# → "indexed N/M session(s) into 'memex_sessions_v3', 0 error(s)"

# 3) CLI smoke test for all 7 surfaces — writes per-surface JSON / TXT
bash scripts/demo/smoke-test.sh --json
# → exit 0 only when every surface returned non-empty
# → files written under tests/e2e/*.{json,txt} (gitignored)

# 4) Deep-link GUI capture — 5 PNGs, one per memex:// route.
#    Requires Memex.app installed (DMG via `npm run tauri build`).
bash scripts/demo/capture-screenshots.sh
# → tests/e2e/screenshots/*.png (gitignored)
```

## Files this directory may contain after a run

| File | Origin |
|---|---|
| `collection-info.json` | `GET /collections/memex_sessions_v3` Qdrant metadata |
| `scan.txt` / `scan.json` | `memex scan --limit 5` (human table → `.txt`; JSON shape → `.json`; extension auto-picked) |
| `search.txt` | `memex search "edit"` dense KNN hits |
| `lens.txt` | `memex lens "edit auth.js"` FormulaQuery hits (P2 KA-01) |
| `topology.json` | `memex topology --sample 30` MST graph (P4) |
| `recall.txt` | `memex recall "cargo build error"` proactive recall hits |
| `predict.txt` | `memex predict <SID> --neighbors 5` next-action prediction |
| `mix.txt` | `memex mix --pos <SID_A> --neg <SID_B>` Discovery API hits |
| `sample-session-id.txt` | scroll first point in v3, anchor for predict / mix |
| `logs/*.log` | run logs from local smoke / capture runs |
| `screenshots/{route}.png` | `open memex://<route>` + region screencapture |

All paths under this directory (except this README, the path itself,
and the scripts in `scripts/demo/`) are matched by `.gitignore`.

## Notes on screenshot capture

`capture-screenshots.sh` reads the Memex main window bounds via
`System Events` and uses `screencapture -R x,y,w,h` to capture only the
window rectangle. This avoids the macOS-14+ regression where
`screencapture -l <window_id>` silently falls back to full-screen.

Between routes the script presses **Escape** to close any open
`<dialog>` so each PNG captures the surface the deep link actually
opened (rather than stacked modals from prior routes). It also resolves
the actual process name from the live PID so dev builds running as
lowercase `memex` work the same as the bundled `Memex.app`.

## What replaces the "committed evidence" goal proof?

The P8 `/goal` END STATE originally required `tests/e2e/screenshots/*.png
→ >= 5 files` checked into git. After the Codex privacy review we
satisfy the equivalent guarantee differently:

- The two helper scripts (`smoke-test.sh`, `capture-screenshots.sh`)
  are committed and tested end-to-end on the contributor's machine.
- A successful local run produces `>= 5` PNGs and 8 JSON/TXT files
  under this directory — verifiable by the reviewer with the
  4-command recipe above.
- The IMPLEMENTATION_REPORT.md continues to record the EMPIRICAL
  results (collection points_count, smoke-test exit code, screenshot
  count) at the time of report, but no longer links the per-file
  artifacts because those leak private data.
