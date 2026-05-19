# Memex · VSD 2026 Demo Video — Shot Script

**Duration**: 180 seconds (3:00) ± 2s
**Format**: 60fps capture (OBS), export 24fps (DaVinci)
**Aspect**: 16:9, 1920×1080 (or 2560×1440 for sharper kerning)
**Voiceover**: optional. App audio = 0; all sound in post.
**Music**: Bensound CC-BY ("Slow Motion" or "Cinematic Documentary"), fade-out for the climax silence.
**SFX**: Kenney UI Audio CC0 — 6 hits total.
**License credit at outro**: Apache-2.0 + Qdrant logo + `github.com/ComBba/memex`.

> Theme: "Think Outside the Bot." Memex turns your existing AI session
> history (Claude Code + Codex CLI) into a spatial memory. No chat box.
> No runtime LLM call. Seven non-chat surfaces over five named vectors per
> session point.

---

## Timeline (18 shots)

### Cold open

```
0:00 ─ Bush 1945 quote fade-in, typed-out
       "Consider a future device for individual use, which is a sort of
        mechanized private file and library… A memex." — Vannevar Bush
       
       Audio: silence (8s)
       Caption: "MEMEX · 1945 → 2026"
```

### Act I — Time Machine + Topology (the spatial foundation)

```
0:08 ─ Memex app launch. Time Machine stack fly-in (60fps card cascade)
       Audio: music drops in (mid pulse) — "Slow Motion" intro
       Caption: "Time Machine · your sessions, stacked in time"

0:18 ─ Close-up on a single card. enrich.rs chip animates in:
        [intent: debug] [arc: debug-fix] [outcome: resolved] [topic: …]
       Audio: chime SFX (Kenney "click_001")
       Caption: "No LLM. Heuristic enrichment, fully local."
       KICK: KD-01-FREE · enrich.rs (P5)

0:28 ─ Stat counter animates:
        "80 sessions · 17,938 tool calls · 0 outbound network bytes"
       Audio: music swells (1st bar)
       Caption: "100% local. fastembed + Qdrant 1.18."
       KICKs: KH-01 multi-agent (P5), KG-03 schema v3 (P3)

0:40 ─ ⌘+T → camera zoom into Topology galaxy (3d-force-graph)
       Cluster auto-labels appear next to each cluster centroid:
        "code+shell · Bash×1350 · 'Tauri build'"
       Audio: music continues
       Caption: "Topology · clusters auto-labeled by enrich.rs"
       KICK: KG-01 insights cache (P5)

0:55 ─ Bridge edge highlight + gap-insight bubble:
        "redesign ↔ yc · sim 0.97 · NO bridge"
       Audio: subtle pulse
       Caption: "Cross-project bridges visible"
       KICK: existing topology pairs (extended in P3)
```

### Act II — Lens + Predict (the retrieval ceiling)

```
1:10 ─ ⌘K opens lens search box. Type: "edit auth.js"
       Split-screen reveal:
         LEFT  · v1: 5 round-trips · 320ms wall clock
         RIGHT · v3: 1 FormulaQuery · 45ms wall clock
       Audio: tick sound on each round-trip; satisfying single click on v3
       Caption: "5 round-trips → 1 server-side formula"
       KICK: KA-01 FormulaQuery (P2)

1:22 ─ Contribution bars animate per result row:
         content 42% · path_sparse 31% · code 14% · recency 0.87 · errors +0.2
       The bars fan out via flex-basis 320ms cubic-bezier transition
       Audio: subtle motion swoosh
       Caption: "Every score, transparently broken down."
       KICK: KA-01 ScoreBreakdown surfacing (P2 → P6 WOW-3)

1:32 ─ Click result → Predict 4×3 thumbnail grid animates in
       Each thumbnail: project · ai_title · top-3 tools · arc badge ·
       outcome chip · 👍/👎
       Click first thumbnail → cinematic zoom (View Transition API)
       Audio: chime; smooth transition
       Caption: "Predict · what would past-you do next?"
       KICK: KG-02 LRU (P5) + KD-01-FREE enrich outcome (P5)
```

### Act III — ⚡ CLIMAX (the reverse query)

```
1:42 ─ CUT to terminal: cargo build fails with red error stack
       Camera SLOW PAN to Memex window in background.
       Music FADES OUT completely.
       SILENCE for the next 12 seconds.

       960 frames at 60fps · all motion paused · zero audio.

       (Caption fades in at 1:43, holds:)
       "What if your past self already solved this?"
```

```
1:54 ─ Recall banner slides in from the top of the Memex window:
       ┌─────────────────────────────────────────────────┐
       │ ⚡ I've seen this before · sim 0.93              │
       │   project: memex · session 7 days ago           │
       │   ACORN-filtered · has_errors=true              │
       └─────────────────────────────────────────────────┘
       Audio: soft single-tone banner SFX (Kenney "switch_004")
       Caption: "Proactive Recall · ACORN filterable HNSW"
       KICK: KB-04 ACORN (P4)

2:00 ─ Music returns at slower tempo (gentle resolution).
       Cut to Mix & Match modal opening.
       Audio: music returns gently
       Caption: "Recover · with Discovery"
```

### Act IV — Discovery + Relevance Feedback

```
2:08 ─ Drag 2 sessions into POSITIVE slot, 1 into NEGATIVE.
       3D hyperplane canvas materializes:
         positive cards on the right of the plane, normal vector
         emerging toward the camera.
       Result cards emit from the plane center.
       Audio: music builds
       Caption: "Discovery · true context pairs"
       KICK: KB-03 Discovery context pairs (P4)

2:22 ─ Click 👍 on the top result. Anchor recomputes; cards re-rank
       instantly with cubic-bezier ease.
       Audio: chime; results re-flow
       Caption: "Relevance Feedback · server-side re-anchor"
       KICK: KA-04 RelevanceFeedback (P4)
```

### Act V — Multi-Agent Reveal (KH-01)

```
2:35 ─ Open Topology again. Toggle the agent filter pill:
         [Both] → [Claude] → [Codex] → [Both]
       In single-agent mode, nodes recolor uniformly.
       In Both mode, the same galaxy now shows both Claude (filled)
       and Codex (open) sessions side-by-side.
       Audio: music climbs
       Caption: "One memory, two agents. Claude + Codex unified."
       KICK: KH-01 multi-agent ingest (v0.4 addendum)
```

### Outro

```
2:48 ─ Title card with kerning:
       ╔═════════════════════════════════════════════╗
       ║                                             ║
       ║              Memex                          ║
       ║      spatial memory                         ║
       ║      Qdrant 1.18 pinnacle                   ║
       ║                                             ║
       ╚═════════════════════════════════════════════╝
       Audio: music resolves
       Caption: ""

2:56 ─ License row:
       github.com/ComBba/memex · Apache-2.0 · Qdrant
       "Think Outside the Bot ✓"
       Audio: final outro chime (Kenney "confirmation_001")
       Caption: VSD 2026
3:00 ─ END
```

---

## Capture checklist (pre-recording)

1. `docker run -d -p 6333:6333 -p 6334:6334 --name memex-qdrant qdrant/qdrant:v1.18.0` — verify `curl http://localhost:6333/readyz`
2. `./src-tauri/target/release/memex scan --index` (first run will download fastembed 130MB)
3. Confirm corpus: `./memex scan --limit 5` shows ≥ 80 sessions across Claude + Codex roots
4. Open `Memex.app` (Tauri-bundled). Grant Full Disk Access at first launch.
5. Pre-stage these queries in clipboard history:
   - `"edit auth.js"` (Act II)
   - `"cargo build error linker"` (Act III recall target)
6. Pre-pin a session card with known recency outliers for the Mix & Match drag
7. Cinema display brightness +2 over baseline (the heat-trail oklch hues need accurate display)

## Pre-record timing pass

Run `scripts/demo/record-demo.sh --dry-run` to verify each shot fits its window. The script does NOT capture — it just prints the cue list with elapsed times so the operator can rehearse cues.

## Post-production cuts (DaVinci)

- Cut all UI-flash artifacts (Tauri webview blink on focus)
- Remove cursor when not the focal point
- Karpathy-style large kerning on captions (FF: Iosevka or IBM Plex Mono, weight 500, letter-spacing +0.05em)
- Color match: clamp shadow lift, lift midtones +5 on the Topology shot (the galaxy reads dark on standard SDR)

## Acceptance criteria (recap)

- [ ] AC-7.1.1 — Length 180s ±2s
- [ ] AC-7.1.2 — 1:42 → 1:54 silence (12s exactly = 720 frames at 60fps or 288 at 24fps)
- [ ] AC-7.1.3 — All 18 shots present
- [ ] AC-7.1.4 — 60fps capture, 24fps export allowed
- [ ] AC-7.1.5 — Outro shows Apache-2.0 + Qdrant logo + repo URL
- [ ] AC-7.1.6 — External blind review (3 people): "this isn't a chatbot" recognized < 30s

## Out of scope (this script)

- Voiceover script (the demo runs without narration; captions only)
- Sound design beyond the 6 Kenney SFX hits (operator's choice)
- Camera moves (single-take pan only; no dolly/zoom keyframes)
