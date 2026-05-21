# VSD 2026 Submission Worksheet — Memex

**Form:** Qdrant's *Think Outside the Bot* Hackathon Project Submission Form
**Form URL:** https://forms.gle/YDQ2TDUi8MqS9Vx28
**Deadline:** 11:59 PM Pacific Time · **Monday, June 1, 2026**
**Canonical repo:** https://github.com/Two-Weeks-Team/memex (PUBLIC ✅)
**Prepared:** 2026-05-21

---

## 🚨 Blocker before you can submit

The form's **"Demo Video Link" is a REQUIRED field (\*)** and "Demo Video is linked above"
is one of the submission-requirement checkboxes. The current task scope **excluded recording the
demo video**, but **the form cannot be submitted without a video link (max 3 min, hosted on
Loom / YouTube / Dropbox).**

→ **Action required from you:** record a ≤3-minute walkthrough (the six surfaces are already
captured as stills in the README Demo section, which can serve as a storyboard) and host it,
then paste the link into field 6 below. Everything else is submission-ready.

---

## 📋 Form fields — dry-run answers

| # | Field (required\*) | Prepared answer |
|---|---|---|
| 1 | **Email** \* | _your contact email_ |
| 2 | **Team Name (or Individual Submitter Name)** \* | `Two-Weeks-Team` |
| 3 | **Project Title** \* | `Memex — AI session history as navigable spatial memory` |
| 4 | **GitHub Repository Link** \* | `https://github.com/Two-Weeks-Team/memex` |
| 5 | **Shared with organizer (@kanungle)?** \* | ✅ **"Yes, the repository is public."** (repo visibility = PUBLIC; no collaborator add needed) |
| 6 | **Demo Video Link (max 3 min)** \* | ⚠️ **PENDING — see blocker above.** Paste Loom/YouTube/Dropbox link once recorded. |
| 7 | **Category/track** \* | ☑ **Infrastructure & Developer Tools** (primary). Optional secondary: Data Visualization & Analytics (the Topology galaxy). |
| 8 | **Brief Abstract / Summary** \* | _see below_ |
| 9 | **Submission requirements included** \* | ☑ README.md present · ☑ Code has basic comments · ☑ Demo Video linked _(check only after field 6 is filled)_ |
| 10 | **Technical Difficulty Rating (1–10)** \* | Suggested **8** — Rust + Tauri 2 desktop app, 5 distinct Qdrant primitives (named vectors, Distance-Matrix API, Discovery API, payload-filtered recall, snapshots), client-side ONNX embeddings, MST topology, zero-LLM recommendation. _Your call._ |

### Field 8 — Abstract (copy-paste ready)

> Memex turns your AI coding history (`~/.claude/projects` + `~/.codex/sessions`) into a
> **navigable spatial memory** instead of yet another chatbot. It indexes every past session into
> Qdrant and exposes **seven non-chat surfaces**: a 3D Time Machine card stack, a force-directed
> **Topology galaxy** (Distance-Matrix API) with cross-project bridge + gap insights, **Mix & Match**
> recommendations (Discovery API), **proactive error recall** (a dedicated `error` named vector +
> `has_errors` payload filter), **next-action prediction** (neighbor pivot-walk + tool-call
> aggregation), turn-by-turn **Replay**, and a weighted **multi-named-vector Lens** search.
> Everything runs **100% locally** in Rust + Qdrant with **zero LLM at runtime** and zero network
> calls. The problem it solves: AI session history is trapped in disposable, linear chat logs you
> can't spatially browse, compare across projects, or learn from — Memex makes that corpus a thing
> you move through.

---

## ✅ Submission readiness checklist

- [x] **Repo public** on canonical `Two-Weeks-Team/memex`
- [x] **README.md** present — install, usage, 6 real screenshots, CLI reference, architecture
- [x] **Code comments** present throughout (Rust + JS)
- [x] **v0.1.0 GitHub Release** published with `Memex_0.1.0_aarch64.dmg` asset (17,119,097 bytes) → https://github.com/Two-Weeks-Team/memex/releases/tag/v0.1.0
- [x] **Gatekeeper / unsigned-MVP** first-launch note in README Download section **and** release notes
- [x] **Demo screenshots** — 6 real surfaces in `docs/img/` + README Demo gallery
- [x] **Build verified** — `npm run tauri:dist` green (devtools-off), `cargo test` 228 passed / 0 failed
- [ ] **Demo video** recorded + hosted + link in form field 6  ← **only remaining blocker**
- [ ] **Form submitted** before 2026-06-01 23:59 PT

---

## v0.1.0 release — ✅ published

Published 2026-05-21 (asset verified via `gh release view`):
**https://github.com/Two-Weeks-Team/memex/releases/tag/v0.1.0**

- Tag `v0.1.0` → `d2bb709` (`main`).
- Asset: `Memex_0.1.0_aarch64.dmg` (17,119,097 bytes).
- DMG sha256: `3408f62250bc052b13b9583ca126d4dd057a59141f68407037f8047c81206a68`

> `gh release create` is gated by the workspace policy hook (manual review), so it was run by the maintainer rather than the agent.
