# Building Memex from source & platform support

The prebuilt release is **macOS Apple Silicon (arm64)** only. This page
documents what's tested, how to build on other platforms, and a CLI / build-only
verification path so a judge can prove the code works without installing the
GUI app.

## Platform support matrix

| Platform | Status | Notes |
|---|---|---|
| **macOS 11+ · Apple Silicon (arm64)** | ✅ Primary, tested | Release DMG + dev builds. Verified on macOS 26.5 / arm64; release binary is `Mach-O arm64`. |
| macOS · Intel (x86_64) | ⚙️ Source build (not tested by us) | No prebuilt DMG. `cargo build --release` + `npm run tauri build` on an Intel Mac should produce an x86_64 app. Use the x86_64 Qdrant tarball or Docker. |
| Linux · x86_64 | ⚙️ CLI from source; GUI needs WebKitGTK | The Rust binary builds on Linux — **exercised by CI** (`.github/workflows/ci.yml`, `ubuntu-latest`). The Tauri GUI additionally needs the WebKitGTK system libs below. No prebuilt artifact. |
| Windows | ❌ Not targeted | The JSONL parser is cross-platform, but packaging/deep-link wiring isn't done. PRs welcome. |

> The **Qdrant dependency is platform-agnostic** via Docker
> (`bash scripts/start-qdrant.sh`), so the only platform-specific part is the
> Tauri GUI shell.

## Prerequisites

- [Rust](https://rustup.rs) 1.88+
- [Node.js](https://nodejs.org) 22+ with npm
- [Qdrant](https://github.com/qdrant/qdrant) 1.18 — `bash scripts/start-qdrant.sh`
- **Linux GUI build only** — the standard Tauri 2 system libraries:
  ```bash
  sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev \
    libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev \
    libxdo-dev libssl-dev build-essential curl wget file pkg-config
  ```
  (These are exactly what CI installs — see `.github/workflows/ci.yml`.)

## Build

```bash
gh repo clone Two-Weeks-Team/memex ~/memex && cd ~/memex
npm install
cargo build --release --manifest-path src-tauri/Cargo.toml
```

This produces **one binary** — `src-tauri/target/release/memex` — that backs
both the CLI and the GUI (it dispatches on `argv[1]`).

## CLI smoke path (no GUI required)

Everything a judge needs to feel the product works from the terminal:

```bash
BIN=./src-tauri/target/release/memex

$BIN --help                                   # subcommand surface
$BIN scan --path examples/sample-corpus       # parse only — no Qdrant needed
bash scripts/start-qdrant.sh                  # bring up Qdrant
$BIN scan --path examples/sample-corpus --index
$BIN search "rate limiter redis"
$BIN lens "build error" --error 2.0 --content 1.0
$BIN recall "cargo build linker error"
$BIN topology --sample 12 --per-point 4 --out /tmp/topo.json
```

See [examples/sample-corpus/README.md](../examples/sample-corpus/README.md)
for expected outputs (and the `predict`/`replay` sandbox note).

## Build-only verification (no Qdrant, no model download)

If you only want to prove the code compiles and the logic is sound:

```bash
cargo build --release --manifest-path src-tauri/Cargo.toml   # compile smoke
cargo test  --manifest-path src-tauri/Cargo.toml             # unit + integration
```

Integration tests that need a live Qdrant self-skip via `skip_if_no_qdrant()`
(env `MEMEX_QDRANT_URL`, default `http://localhost:6334`), so the suite is
**green without Qdrant** — this is exactly what CI relies on. Run them for real
by starting Qdrant first. Exact test counts and the last local run are recorded
in [docs/e2e-evidence.md](e2e-evidence.md).

## GUI build (macOS)

```bash
npm run tauri build    # local Memex.app (WebKit Inspector ON)
npm run tauri:dist     # distribution .dmg via --no-default-features (Inspector OFF)
```

## Non-arm64 Mac (cross / native)

- **Native (recommended):** build on the target Mac directly with the commands
  above.
- **Cross-target:** `rustup target add x86_64-apple-darwin` then
  `cargo build --release --target x86_64-apple-darwin --manifest-path src-tauri/Cargo.toml`.
  Bundling an x86_64 `.app`/`.dmg` this way is **untested by us** — report
  results in an issue.
