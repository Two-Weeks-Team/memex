# Memex v2 — Handoff to Fresh Session

> **Read this first in the new Claude Code session.**

## What Memex is

**Memex** is a macOS Time Machine for AI session JSONL files (`~/.claude/projects/*.jsonl`).
It indexes them with Qdrant and provides 5 features no other vector DB can easily ship:

1. **Lens slider** — 5 named vectors (content/tool/path/error/code) weighted in one Universal Query
2. **Mix & Match** — Discovery API multi-pair context (combine 2 sessions, exclude 1)
3. **Topology view** — Distance Matrix → MST graph of all sessions
4. **ColBERT inline citation** — sentence-level highlight with line numbers
5. **Snapshot** — portable index export via Qdrant snapshot API

Plus: **Replay engine** (watch past sessions turn-by-turn) and **proactive recall** (file watcher detects recurring errors).

Target: Qdrant Vector Space Day 2026 (deadline 2026-06-01).

## Where we are (2026-05-18)

- ✅ Concept locked: Memex (after pivoting from Atlas Plan B/B-prime, both validation-failed)
- ✅ UI mockup complete: `mockups/memex/demo.html` (v2 macOS HIG) + `landing.html`
- ✅ Tech stack decided: Tauri 2.x + Qdrant + FastEmbed (no Python sidecar)
- ✅ Repo decided: standalone `memex` on personal GitHub
- ✅ Privacy: user authorized full `~/.claude/projects` scan
- ⏳ **Implementation not yet started** — that's your job

## What you (new session) do

1. Read `docs/memex/PLAN.md` (in the myproject worktree at `/path/to/workspace-b/docs/memex/PLAN.md`)
2. Follow the ralph_loop prompt at §7 of PLAN.md
3. Iterate through TODO checklist (§4)
4. Pause at CLI intervention points (§5)
5. Pass test gates (§6)
6. Ship by ~May 25 for buffer

## How to start

### Option A — Use the ralph_loop skill (recommended)

In a fresh session, paste this:

```
Read /path/to/workspace-b/docs/memex/PLAN.md
in full. It is the single source of truth for the Memex implementation.

Then execute the ralph_loop prompt described in §7 of that document.

Your job is to drive the implementation to completion, pausing only at
CLI intervention points (§5) where the user must act.
```

### Option B — Manual / autonomous-loop

```
/loop "Read docs/memex/PLAN.md and execute the next unchecked task. Update PLAN.md to mark it done. Commit. Pause if it's a user-CLI task."
```

## Critical context (do not lose)

- **myproject worktree** at `/path/to/workspace-b/` is REFERENCE ONLY. Do not modify it (except docs/memex/ updates).
- **memex repo** will be at `~/memex/` (created by T0.1)
- **Mockups** at `mockups/memex/{demo,landing}.html` are CANONICAL UI. Port to Tauri webview.
- **Atlas spike results** (`spikes/09_signal_validation/`) are NOT relevant to Memex. Ignore.
- **4,493 Devpost data** (`data/devpost-gemini3/`) is NOT relevant to Memex. Ignore.

## What's already learned (don't re-discover)

1. **Atlas Plan B-prime falsified**: 0/13 winner recovery on 4,493 corpus. We're not doing that.
2. **8 existing competitors** for session search exist (ccsearch, agent-traces, claude-vector-db, etc.) but NONE do the 5 Qdrant-specific features as visible product UI. That's our moat.
3. **macOS Time Machine layered card UX** is the right UI. Tesseract/cosmic abstract was rejected by user.
4. **5 Qdrant features as product** is the differentiation, NOT just "search sessions" (which was the lazy approach).

## What can go wrong + escape hatches

| Problem | Action |
|---|---|
| ralph_loop wanders | Stop. Re-read PLAN.md. Resume from last unchecked task. |
| qdrant-client Rust missing Discovery/Distance-Matrix | Fall back to REST API via `reqwest` |
| FastEmbed lacks ColBERT v2 | Use `ort` Rust crate to embed locally |
| Tauri build fails on macOS | Verify Xcode CLT, then check tauri.conf.json |
| File watcher unreliable | Fall back to 2s polling |
| Demo video looks bad | Re-record, multiple takes |
| Stuck >30 min | STOP, ask user |

## Resume signal

If a future session needs to know "where are we?":
- Check `docs/memex/PLAN.md` §4 for last `[x]` checked task
- Or `cd ~/memex && git log --oneline -20`
- Most recent commit message tells you which phase/task

---

*2026-05-18 — handoff written in myproject worktree.*
