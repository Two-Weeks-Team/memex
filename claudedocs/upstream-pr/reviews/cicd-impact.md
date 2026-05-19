# CI/CD & Build Impact Review — Upstream PR Candidates

**Reviewer**: DevOps architect agent (parallel #4 of 7)
**Date**: 2026-05-19 (D-13)
**Scope**: 6 candidate commits proposed for `sgwannabe/memex` upstream
**Fork HEAD**: `8509096` · **Upstream HEAD**: `4973a91`
**Reference doc**: `/Users/kimsejun/Documents/GitHub/memex/claudedocs/UPSTREAM_PR_PLAN.md` §2

---

## §0 TL;DR

- **Upstream has zero CI today.** No `.github/workflows/` directory exists in `upstream/main` at any point in history (`git log upstream/main -- .github` is empty). Acceptance is purely manual `cargo build --release` + visual inspection by the maintainer.
- **No `Dockerfile` / `docker-compose.yml` in either tree.** Qdrant is assumed to be run by the user out-of-band (per upstream `README.md` "Install & Run" section). PRs do not change this — no infra files are touched.
- **No fork PR introduces a non-MIT/Apache dep.** All new crates (`sha2`, `notify`, `notify-debouncer-full`, `num_cpus`, `lru`, `dirs`, `tempfile`, `tauri-plugin-deep-link`) are MIT or MIT-OR-Apache-2.0.
- **One PR is a CI/release liability**: `84db1fc` adds `"devtools"` to `tauri = { features = ... }` **unconditionally**, which ships the WebKit Web Inspector inside release `.app` bundles. This must be feature-gated before upstream merge — see §3.
- **Platform coverage**: every candidate is portable in principle, but the fork is tested only on macOS aarch64 (per `tauri.conf.json` `bundle.macOS.minimumSystemVersion: "11.0"` and upstream README badge `macOS 11+ (Apple Silicon)`). Linux/Windows are theoretical surfaces — recommend a starter CI matrix (§4).
- **Only `e402b1f` and `deed283` are zero-build-impact** (frontend-only). `f55d417`, `2b59dc9`, `e1c075b`, `84db1fc` all touch Rust and require a fresh `cargo build --release` cycle (~3–5 min cold on M-series, ~20–30 s incremental).

---

## §1 Inventory of CI / Build Config

### 1.1 Workflow files

| Location | Upstream `4973a91` | Fork `8509096` |
|---|---|---|
| `.github/workflows/*` | **absent** | **absent** |
| `.github/*` (any) | **absent** | **absent** |
| `Dockerfile` | absent | absent |
| `docker-compose.yml` | absent | absent |
| `tauri-action` usage | none | none |

`git log upstream/main --oneline -- .github` returns no commits — upstream has never had GitHub Actions. The hackathon scope on both sides is "build locally on M-series Mac, distribute the `.app` by hand."

### 1.2 `package.json` (root)

Identical, byte-for-byte (`git diff upstream/main main -- package.json` is empty):

```json
{
  "name": "memex",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": { "tauri": "tauri" },
  "devDependencies": { "@tauri-apps/cli": "^2" }
}
```

- No `build` / `dev` / `test` scripts — `pnpm tauri build` is the entire pipeline.
- No frontend bundler (vite / webpack). `tauri.conf.json` points `frontendDist: "../src"`, i.e. the raw HTML/CSS/JS in `src/` is served directly. **No npm transitive deps** outside `@tauri-apps/cli`.
- → **CI impact of any candidate on the npm side is zero.**

### 1.3 `src-tauri/Cargo.toml` divergence (fork vs upstream)

`git diff upstream/main main -- src-tauri/Cargo.toml`:

```diff
-tauri = { version = "2", features = ["tray-icon"] }
+tauri = { version = "2", features = ["tray-icon", "devtools"] }
 tauri-plugin-opener = "2"
-tauri-plugin-notification = "2"
+tauri-plugin-deep-link = "2"
 ...
+notify = "6"
+notify-debouncer-full = "0.3"
+sha2 = "0.10"
+num_cpus = "1"
+lru = "0.12"
+dirs = "5"

 [dev-dependencies]
 pretty_assertions = "1"
+tempfile = "3"
```

Net delta on the dependency graph (`git diff upstream/main main -- src-tauri/Cargo.lock | grep '^+name' | sort -u`):

- 34 new transitive crate names enter the lockfile, dominated by `notify` (file-watcher: `fsevent-sys`, `inotify`, `kqueue`, `mio`, `file-id`, `filetime`) and Windows targets pulled in by `dirs` / `notify` (`windows-registry`, `windows-sys`, `windows-targets`, etc.).
- All licenses verified MIT or MIT-OR-Apache-2.0 — no GPL / LGPL / proprietary entries.

### 1.4 `src-tauri/tauri.conf.json` divergence

`git diff upstream/main main -- src-tauri/tauri.conf.json`:

```diff
+  "plugins": {
+    "deep-link": {
+      "mobile": [],
+      "desktop": { "schemes": ["memex"] }
+    }
+  }
```

This is the only structural change. `bundle.targets: "all"`, `productName`, `identifier: "dev.sgwannabe.memex"`, and `macOS.minimumSystemVersion: "11.0"` are identical.

### 1.5 `src-tauri/capabilities/default.json`

```diff
-    "notification:default"
+    "deep-link:default"
```

**Regression risk if a candidate is cherry-picked naively**: the fork removed the `notification:default` permission when it dropped `tauri-plugin-notification` (per fork commit `81a6c8b chore: drop dead notify deps + standardise recall event`). Upstream still ships notifications via that plugin. A candidate that needs notifications upstream-side must re-add the permission.

### 1.6 `src-tauri/build.rs`

```rust
fn main() { tauri_build::build() }
```

Identical on both sides. No build-script divergence to worry about.

### 1.7 `.gitignore` (root) — fork adds

```diff
+tests/e2e/*.json
+tests/e2e/*.txt
+tests/e2e/screenshots/*.png
+tests/e2e/screenshots/*.jpg
+tests/e2e/logs/
```

These rules cover fork-only `tests/e2e/` artifacts that upstream doesn't ship. Harmless to include in a PR; CI would simply never see those paths.

### 1.8 CI invariants the fork implicitly assumes that upstream may not

| Invariant | Fork status | Upstream status | Risk |
|---|---|---|---|
| `pnpm`/`npm` available + `@tauri-apps/cli@^2` resolves | yes | yes | none |
| Rust toolchain `1.77+` (Tauri 2 MSRV) | implicit | implicit | none |
| `cargo build --release` produces a working `.app` on macOS 11+ aarch64 | tested daily | upstream README badge implies same | none |
| Qdrant running on `localhost:6334` at runtime | required for runtime, not build | same | none |
| `dirs::data_dir()` resolves on the target platform | required by `sec.rs` + `snapshot.rs` | upstream lacks both files; only matters if `f55d417` is merged | low |
| `~/.codex/sessions` may not exist | fork degrades gracefully | upstream doesn't have the code at all | n/a |
| `notify` fsevent backend available on macOS | required by `watcher.rs` (fork-only, removed from upstream in `81a6c8b`) | not used | n/a |

---

## §2 Per-Candidate CI Impact Matrix

Legend: ⚪ no impact · 🟢 small/safe · 🟡 needs review · 🔴 blocks merge.

| Commit | Subject | Rust touched | New crates | License OK | Build-time Δ | Binary size Δ | Env vars / secrets | Platform diffs | Test runtime Δ | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| `e402b1f` | mix-modal self-contained picker | no (HTML/JS/CSS) | none | n/a | 0 (no rebuild needed) | 0 | none | uses `<dialog>` (full support in WebKit 12.1+, fine on macOS 11+); same in Linux WebKit & Edge | 0 | 🟢 |
| `f55d417` | P1 security (sandbox + signed snapshot) | yes (`+sec.rs`, `+snapshot.rs`, edits `commands.rs`, `indexer.rs`, `lib.rs`) | `sha2 = 0.10`, `dirs = 5`, `tempfile = 3` (dev) | MIT/Apache-2.0 all three | +~5–10s cold (`sha2` is pure-Rust, no C deps); +~3s incremental | +~80–120 KB (`sha2` static) | none new | `dirs::home_dir()` returns `None` on Android/iOS — irrelevant for desktop Tauri; sandbox path math is platform-portable | +30 unit + 4 integration tests, ~1–2s | 🟡 — see §2.f |
| `2b59dc9` | predict: route Codex through `codex_parser` | yes (`indexer.rs` only) | none | n/a | minimal (one fn body change) | 0 | none | none | 0 (no new tests) | 🔴 — depends on `codex_parser.rs` which isn't in upstream; standalone cherry-pick won't compile |
| `deed283` | kill heat-trail purple oval | no (`main.js`, `styles.css`) | none | n/a | 0 | 0 | none | uses SVG `vector-effect="non-scaling-stroke"` — WebKit support since 2017 ✅ | 0 | 🔴 — touches `drawHeatTrail()` which lives in fork-only `lens.rs`/JS code; the file exists upstream but the function doesn't |
| `e1c075b` | CLI: ensure v3 collection before bulk index | yes (`cli.rs` only) | none | n/a | minimal | 0 | none | none | 0 | 🔴 — depends on `crud::ensure_collection_v3()` and `schema::COLLECTION_V3` symbols that don't exist in upstream |
| `84db1fc` | devtools + recall filter + tooltip | yes (`Cargo.toml` 1 line, `commands.rs` filter) | enables `tauri/devtools` feature | upstream-owned feature, no new crate | +~30s cold (compiles `tauri`'s devtools subcrates) | **+~8–12 MB** on release `.app` (Web Inspector bundle) | none | macOS opens Inspector via right-click; Windows uses F12; Linux uses WebKitGTK's `WEBKIT_INSPECTOR_SERVER` env. **Should NOT ship in release upstream** | 0 | 🔴 — see §3 |

### 2.a `e402b1f` (Mix modal picker)

- **Pure frontend.** Edits `src/index.html`, `src/main.js`, `src/styles.css`. No `cargo` rebuild required.
- The HTML structure of `#mix-modal` differs upstream (per UPSTREAM_PR_PLAN §2 trial cherry-pick), so this lands in upstream via a hand-written equivalent rather than a clean cherry-pick. CI-wise: no impact whatsoever.

### 2.b `f55d417` (P1 security)

- **Three new runtime crates**: `sha2 = "0.10"` (MIT/Apache-2.0), `dirs = "5"` (MIT/Apache-2.0). `tempfile = "3"` is dev-only.
- `num_cpus = "1"` and `lru = "0.12"` arrive in this same commit's Cargo.toml block but are actually used by later P5 code — if `f55d417` is split out for upstream, drop those two lines or accept slight binary bloat (~30 KB).
- Build-time impact is small. `sha2` compiles in <5s. `dirs` is essentially zero-cost (already a transitive dep via `tauri` itself).
- 30 unit + 4 integration tests run in ~1–2 s. They are hermetic (use `tempfile`) — safe to run in any CI environment without network/filesystem permissions beyond `$TMPDIR`.
- **Platform note**: `dirs::data_dir()` returns `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` on Linux, `%APPDATA%` on Windows. `SnapshotSandbox::validate_path` therefore differs across OSes — fine, but the hard-coded fixture path in `snapshot_integration.rs` should use `dirs::data_dir()` lookups too (verify before upstream submission).

### 2.c `2b59dc9` (predict Codex routing)

- Single-file change in `indexer.rs` (`predict_next_actions`).
- **Compile-time blocker upstream**: the fix routes to `codex_parser::parse_session_meta` and reads `payload.source_agent`. Upstream has:
  - no `codex_parser.rs` module
  - no `source_agent` field on the payload struct
  - only one parser (`parser::parse_session` for Claude JSONL)
- Net effect: the patch is conceptually correct but mechanically un-cherry-pickable. Would need to be re-authored once upstream gains Codex support (i.e. as part of a larger feature drop, not a standalone PR).

### 2.d `deed283` (heat-trail oval)

- Frontend only — `src/main.js` (+57/-7), `src/styles.css` (+8/-1).
- The `drawHeatTrail()` JS function being patched **does not exist upstream**. The whole WOW-1 heat-trail visualisation came in fork P5/P6.
- → cannot be merged upstream until the heat trail itself is merged. Until then this is a fork-only safety net.

### 2.e `e1c075b` (CLI v3 collection)

- One-line dep change (`use crate::{codex_parser, crud, indexer, parser};` adds `crud`). The bug-fix body calls `crud::ensure_collection_v3()` and uses `schema::COLLECTION_V3`.
- **Compile-time blocker upstream**: `crud.rs` (564 LoC) and `schema.rs` (854 LoC) are fork-only files (introduced in P3 KG-03 dual-write). Upstream still has the single-collection model.
- Same status as `2b59dc9` — correct fix, but only meaningful inside the P3 feature drop.

### 2.f `84db1fc` (devtools + recall + tooltip)

- **Recall filter** (`commands.rs::tail_recent_errors`) and **tooltip** (`main.js`) are clean bug fixes — safe and isolated, no new deps, no platform diffs.
- **Devtools feature** (`Cargo.toml: tauri = { features = ["tray-icon", "devtools"] }`) is the problem:
  - Tauri's `devtools` feature enables the WebKit Web Inspector **even in release builds**, deliberately.
  - On macOS aarch64 this adds **~8–12 MB** to the bundled `.app` and exposes a remote debugging surface that any local code (or a malicious link) can probe.
  - Fork acknowledges this in the commit body: *"adds a small WebView inspector bundle in the release binary (~5-10 MB delta on macOS aarch64). Worth it for empirical debugging during the D-day push; can be turned off again for the hackathon final binary if size matters."*
  - For upstream this is unacceptable. See §3 for the recommended gate.

---

## §3 `84db1fc` — Devtools-in-prod CI/Release Hardening

### Problem

`src-tauri/Cargo.toml` line 21 (fork):

```toml
tauri = { version = "2", features = ["tray-icon", "devtools"] }
```

`"devtools"` is unconditional. `cargo build --release` produces a binary with Web Inspector enabled. Upstream cannot accept this as-is because:

1. **Release-binary surface area**: Web Inspector can read DOM, evaluate arbitrary JS, dump localStorage / IndexedDB, hit `__TAURI__` IPC. In a release app this is a privilege escalation primitive for any process that can already inject into the WebView (browser extensions on Linux/WebKitGTK, accessibility tools on macOS).
2. **Binary bloat**: 8–12 MB out of a ~25 MB total `.app` is ~40% size growth for a feature 99 % of users won't use.
3. **No opt-out path**: There's no env var or runtime flag that gates this; it's a compile-time feature.

### Recommended pattern — Cargo feature

Replace the unconditional flag with an opt-in Cargo feature:

```toml
# src-tauri/Cargo.toml
[features]
default = []
# Enables the WebKit Web Inspector inside the release binary. OFF by default
# because it bloats the bundle ~10 MB and exposes a remote-debug surface.
# CI release builds MUST NOT pass this. Local devs enable it ad-hoc with
#   cargo tauri build --features devtools
devtools = ["tauri/devtools"]

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
```

Then `cargo tauri dev` / `cargo run` automatically gets devtools via Tauri 2's built-in debug-build behaviour (Tauri already enables the inspector in `cargo run` / `cargo tauri dev` even without the `devtools` feature — that's the whole point of the feature flag: it's *only* needed to turn the inspector on in `--release`).

### Alternative pattern — `cfg(debug_assertions)`

If a Cargo feature is too heavyweight, the same effect via build profile:

```toml
[target.'cfg(debug_assertions)'.dependencies]
# In debug builds the inspector is already on; this is redundant
[target.'cfg(not(debug_assertions))'.dependencies]
# Release builds: no devtools feature → no inspector
```

This is uglier and harder to override locally; the Cargo feature is preferred.

### Recommended commit body for the upstream PR

When the fork resubmits `84db1fc` upstream, the Cargo.toml hunk should look like:

```diff
+[features]
+default = []
+devtools = ["tauri/devtools"]
+
 [dependencies]
 tauri = { version = "2", features = ["tray-icon"] }
```

And the PR description should explicitly mention:

> Devtools are now an opt-in Cargo feature, not a hard-coded `--release` flag. Use `cargo tauri build --features devtools` to produce a bundle with the inspector enabled (for diagnosing rendering bugs against a real `.app`). Default release builds remain devtools-free.

### CI guardrail

Add (in §4 below) a one-line check to the proposed release workflow:

```yaml
- name: Verify release bundle does NOT include devtools
  run: |
    ! grep -R 'devtools' src-tauri/target/release/bundle/macos/*.app/Contents/Frameworks/ \
      || (echo "devtools shipped in release binary" && exit 1)
```

(This is heuristic — the inspector resources have recognisable filenames inside the bundled WebKit frameworks. A more reliable check is `! cargo metadata --format-version 1 | jq '.packages[] | select(.name == "tauri") | .features | .devtools'` against the resolved manifest before `cargo tauri build`.)

---

## §4 Recommended GitHub Actions for Upstream

Upstream has no CI today, so a minimal starter workflow benefits any of these PRs being merged. Two files cover 95 % of useful coverage.

### 4.1 `.github/workflows/build.yml` — PR gate

```yaml
name: build
on:
  pull_request:
  push:
    branches: [main]

jobs:
  rust-check:
    runs-on: macos-14   # M-series, matches upstream README "macOS 11+ (Apple Silicon)"
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: src-tauri
      - name: cargo fmt
        run: cargo fmt --manifest-path src-tauri/Cargo.toml --check
      - name: cargo clippy
        run: cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
      - name: cargo test (lib)
        run: cargo test --manifest-path src-tauri/Cargo.toml --lib
      - name: cargo test (integration)
        run: cargo test --manifest-path src-tauri/Cargo.toml --tests
      - name: Verify devtools feature is opt-in
        run: |
          cd src-tauri
          # Fail if `tauri` is resolved with devtools by default
          cargo metadata --format-version 1 \
            | jq -e '.resolve.nodes[] | select(.id | startswith("tauri ")) | .features | index("devtools") == null' \
            > /dev/null
```

Why `macos-14` (aarch64) specifically: upstream README badges `macOS 11+ (Apple Silicon)`; `tauri.conf.json` sets `minimumSystemVersion: "11.0"`; the Rust crate `fastembed` ships precompiled ORT binaries that need the aarch64 dylib path resolved on macOS. A `ubuntu-latest` runner would diverge from production immediately.

### 4.2 `.github/workflows/release.yml` — tag-driven `.app` artefact

```yaml
name: release
on:
  push:
    tags: ['v*']
  workflow_dispatch:

jobs:
  build-app:
    runs-on: macos-14
    permissions:
      contents: write  # for GitHub release upload
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: pnpm/action-setup@v3
        with: { version: 9 }
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      - uses: Swatinem/rust-cache@v2
        with: { workspaces: src-tauri }
      - run: pnpm install --frozen-lockfile
      - name: Tauri build (no devtools)
        run: pnpm tauri build
      - name: Verify devtools NOT bundled
        run: |
          cd src-tauri/target/release/bundle/macos
          ! find . -name '*Inspector*' -print -quit | grep -q . \
            || (echo "Inspector shipped in release bundle — abort" && exit 1)
      - name: Upload .app artefact
        uses: actions/upload-artifact@v4
        with:
          name: Memex-macos-aarch64
          path: src-tauri/target/release/bundle/macos/Memex.app
      - name: Attach .dmg to GitHub release
        if: startsWith(github.ref, 'refs/tags/v')
        uses: softprops/action-gh-release@v2
        with:
          files: src-tauri/target/release/bundle/dmg/Memex_*.dmg
```

Notes:
- No code-signing / notarisation step. Upstream isn't enrolled in the Apple Developer Program (per public repo metadata), so the artefact is an unsigned `.app`. Users see Gatekeeper warning on first launch. Adding signing later is an `APPLE_CERTIFICATE` / `APPLE_ID` secrets workflow — out of scope for these PRs.
- The `fastembed` crate downloads ONNX models at first runtime, not build time. Build doesn't need network.
- Qdrant is **not** spun up in CI. Tests are hermetic (verified above).

### 4.3 Optional `.github/workflows/qdrant-smoke.yml` — integration test

Only worthwhile if the maintainer wants end-to-end coverage of the indexer/retrieval path. Spins up Qdrant in a service container:

```yaml
name: qdrant-smoke
on: { workflow_dispatch: {} }
jobs:
  smoke:
    runs-on: macos-14
    services:
      qdrant:
        image: qdrant/qdrant:v1.13.1
        ports: ['6333:6333', '6334:6334']
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo run --manifest-path src-tauri/Cargo.toml -- scan --root tests/fixtures --index
      - run: cargo run --manifest-path src-tauri/Cargo.toml -- search "hello" --topk 3
```

(macOS runners don't natively support `services:` containers — this would need to be `ubuntu-latest` with `--target x86_64-unknown-linux-gnu` skipping the Tauri shell. Listed as a "nice-to-have" only.)

---

## §5 Release-Notes Blurbs (one-liners for CHANGELOG)

These are written as if landing upstream with conflicts resolved manually.

| Commit | CHANGELOG line |
|---|---|
| `e402b1f` | **Fixed** — Mix & Match modal is now self-contained: search and add sessions from inside the dialog instead of needing pre-staged `+ pos / − neg` clicks on cards behind the backdrop. |
| `f55d417` | **Added** — Path sandbox and signed snapshot envelope (`sha256` sidecar + schema-version check) for session-rollout reads and snapshot import/export. Cross-platform via `dirs::data_dir()`. 34 new tests. |
| `2b59dc9` | **Fixed** — `memex predict` against Codex sessions now routes through the Codex JSONL parser instead of falling back to the (empty-result) Claude parser. *(Requires the P5 Codex-parser drop to be in tree first.)* |
| `deed283` | **Fixed** — Heat-trail SVG no longer renders a viewport-spanning "purple oval" on card hover: stroke-width is clamped, `vector-effect="non-scaling-stroke"` keeps the line crisp across viewBox scales, and a viewBox sanity gate prevents 0×0 explosions. *(Requires the heat-trail visualisation to be in tree first.)* |
| `e1c075b` | **Fixed** — `memex scan --index` provisions the v3 collection before bulk-indexing, so a fresh Qdrant no longer produces N error responses on first run. *(Requires the P3 dual-write schema in tree first.)* |
| `84db1fc` | **Added** — Optional `devtools` Cargo feature exposes the WebKit Inspector in release builds for empirical UI debugging (opt-in, off by default). **Fixed** — `tail_recent_errors` no longer surfaces shell-stderr noise (jq syntax errors, `command not found`, etc.) in the recall banner; only structured `is_error: true` tool results count. **Changed** — `has_errors` badge on session cards has an explanatory tooltip. |

---

## §6 Rollback Plan (per PR)

Format: **what** to revert + **observability** to catch the issue first.

### `e402b1f` — Mix modal

- **Revert**: `git revert <merge-commit>` — clean, frontend-only, no DB/disk side effects.
- **Detect regression**: user opens Mix & Match, can't see search results inside the modal → file bug. No log to grep (frontend only). Recommend a `console.warn` if `mix-picker` container fails to render.

### `f55d417` — Security sandbox

- **Revert**: `git revert <merge-commit>`. Side effects to clean up: any `.snapshot.sig` files left in `~/Library/Application Support/dev.sgwannabe.memex/snapshots/` become orphaned but harmless (the legacy import path ignores unknown sidecars).
- **Detect regression**: error rate on `validate_session_path` spikes in user reports. Add a one-time `eprintln!("[sec] sandbox-reject: {}", err)` (already in fork) — pipe to `~/Library/Logs/` for telemetry.
- **Partial rollback** option: keep `KF-01` (path containment) and revert `KF-02/03` (snapshot signing) by reverting only `snapshot.rs` and the `commands::snapshot_*` wiring.

### `2b59dc9` — Codex predict

- **Revert**: `git revert <merge-commit>`. Single-file change to `predict_next_actions` — trivial.
- **Detect regression**: `memex predict <codex-SID>` returns "looked at 0 similar session(s)" → user complaint → grep `[predict] empty-neighbours` in indexer logs.

### `deed283` — Heat-trail

- **Revert**: `git revert <merge-commit>`. Pure CSS/JS; effect is purely cosmetic.
- **Detect regression**: "giant purple oval on hover" returns. Add a runtime guard `if (strokeWidth > 5) console.warn('heat-trail stroke clamp bypassed')` for early detection. Screencap proof already in `claudedocs/reports/purple-oval/`.

### `e1c075b` — CLI v3 collection

- **Revert**: `git revert <merge-commit>`. Reverts the `crud::ensure_collection_v3` call site in `cmd_scan`.
- **Side effect to clean up**: if users have already run `scan --index` post-PR they'll have a `memex_sessions_v3` Qdrant collection. Reverting won't delete it (Qdrant collections survive client lifecycle); recommend a documented `curl -X DELETE localhost:6333/collections/memex_sessions_v3` rollback step.
- **Detect regression**: `scan --index` reports `N errors / 0 indexed` on fresh Qdrant.

### `84db1fc` — Devtools + recall + tooltip

This is three logical changes; revert them independently:

1. **Devtools feature**: `git revert -n <commit> && git checkout HEAD -- src-tauri/src/commands.rs src/main.js && git commit -m 'revert: devtools-in-release only'`. Reverts only the Cargo.toml line; recall filter and tooltip stay.
2. **Recall filter**: revert only `commands.rs::tail_recent_errors` block.
3. **Tooltip**: revert only `main.js` `errBadge` line.
- **Detect regression** (devtools): a CI grep of release `.app` finds `Inspector.framework` symbols → fail the build (see §3 CI guardrail).
- **Detect regression** (recall): jq-stderr noise reappears in recall banner — user-visible, no instrumentation needed.

---

## §7 Concrete CI Invariants the Fork Implicitly Assumes That Upstream Does Not Enforce

Submit-side checklist for any of these PRs to be safely accepted:

1. **`cargo build --release` on macOS 14 aarch64 must produce a `.app` < 35 MB.** Today fork is ~30 MB; `84db1fc` (with devtools unconditional) pushes to ~40 MB. The proposed Cargo-feature gate keeps default below 35 MB.
2. **`cargo test --tests` must complete < 30 s.** Fork integration tests take 22 s today; `f55d417`'s 4 integration tests add ~2 s. Safe.
3. **No new top-level files outside `src/`, `src-tauri/`, `docs/`.** All candidates honour this except `f55d417` which adds `src-tauri/tests/sec_integration.rs` and `src-tauri/tests/snapshot_integration.rs` — correct location, no policy violation.
4. **`tauri.conf.json` `identifier` must remain `dev.sgwannabe.memex`.** All candidates honour this.
5. **No new licenses outside MIT/Apache-2.0/BSD-3.** Verified for all 34 new transitive crates introduced by the fork's `Cargo.lock` delta.
6. **No new environment variables required at runtime.** All candidates honour this — Qdrant connection is configured at the existing `localhost:6334` default; `dirs::data_dir()` resolves automatically.
7. **No new secrets in CI.** None of these PRs need an API key, signing cert, or container registry credential.

---

## §8 Summary by Candidate

| Commit | Submit upstream now? | Blocker (if any) | CI risk |
|---|---|---|---|
| `e402b1f` | ⚠️ Re-author manually (HTML divergence per UPSTREAM_PR_PLAN §2) | none for CI | none |
| `f55d417` | ⚠️ Large; split `KF-01` (sandbox) from `KF-02/03` (signed snapshot) | review API surface change to `snapshot_import` return type | low — 3 new crates, all MIT/Apache |
| `2b59dc9` | ❌ Hold | depends on P5 Codex parser drop | n/a until P5 lands |
| `deed283` | ❌ Hold | depends on heat-trail visualisation (P5/P6 feature drop) | n/a |
| `e1c075b` | ❌ Hold | depends on P3 dual-write schema drop | n/a |
| `84db1fc` | ⚠️ Strip devtools or feature-gate it (see §3); recall filter and tooltip can go alone | unconditional `tauri/devtools` in release builds | high without §3 fix |

**Bottom line**: only `e402b1f`, `f55d417` (split), and a devtools-gated `84db1fc` are realistic standalone upstream PRs. The other three are feature-drop-dependent.
