# Installing Memex

A step-by-step install guide. It is written so a **coding agent** (Claude Code,
Codex, Cursor, …) can follow it end to end, but a human can run the same
commands. Pick one install path (A, B, or C), then do the shared setup.

- **A — Homebrew** (macOS, Apple Silicon): the fastest path; installs the signed,
  notarized app.
- **B — DMG**: same signed app, downloaded directly.
- **C — Build from source** (macOS): produces the CLI + GUI from source.
- **Headless / non-macOS**: see [the Docker `web` variant](../deploy/web/README.md)
  (Qdrant + web UI/API + MCP in one container, runs anywhere).

Memex needs a local **Qdrant 1.18** on `localhost:6334`. It self-heals if Qdrant
starts after the app, but start it first to avoid an empty first screen.

---

## A — Homebrew (recommended)

```bash
brew install --cask two-weeks-team/tap/memex
# upgrade later:
brew upgrade --cask memex
```

The cask installs `Memex.app` (signed with a Developer ID and notarized, so it
opens without a Gatekeeper warning). Continue at [Shared setup](#shared-setup).

## B — DMG (direct download)

```bash
open "https://github.com/Two-Weeks-Team/memex/releases/latest"
# Download Memex_0.1.2_aarch64.dmg, open it, drag Memex.app to /Applications.
```

The DMG is signed and notarized — a normal double-click works; no quarantine
workaround is needed. Continue at [Shared setup](#shared-setup).

## C — Build from source (macOS, Apple Silicon)

Prerequisites: [Rust](https://rustup.rs) 1.88+, [Node](https://nodejs.org) 22+,
and [Qdrant](https://github.com/qdrant/qdrant) 1.18.

```bash
gh repo clone Two-Weeks-Team/memex ~/memex && cd ~/memex
npm install
cargo build --release --manifest-path src-tauri/Cargo.toml   # CLI + GUI binary
npm run tauri build                                          # produces a local Memex.app
open src-tauri/target/release/bundle/macos/Memex.app
```

The same binary is the CLI; put it on PATH for the commands below:

```bash
export PATH="$PWD/src-tauri/target/release:$PATH"
```

See [BUILD.md](BUILD.md) for Intel/Linux notes and a CLI-only smoke path.

---

## Shared setup

### 1. Start Qdrant

```bash
bash scripts/start-qdrant.sh                 # starts qdrant/qdrant:v1.18.1 on :6334 (gRPC) / :6333 (REST)
curl -fsS http://localhost:6333/readyz && echo OK
```

(If you installed the app via Homebrew/DMG and don't have the repo, run Qdrant
with Docker: `docker run -d -p 6333:6333 -p 6334:6334 qdrant/qdrant:v1.18.1`.)

### 2. Grant Full Disk Access (GUI app only)

Memex reads `~/.claude/projects` and `~/.codex/sessions` locally. On recent
macOS, grant **Full Disk Access** to `Memex.app` in **System Settings → Privacy
& Security → Full Disk Access**, then relaunch. Nothing leaves your machine —
all parsing, embedding, and search are local.

### 3. Index your sessions

```bash
memex scan --index            # downloads the BGE-small model (~130 MB) on first run
#   parsed N session(s), … total tool calls
#   indexed M/N session(s) into 'memex_sessions_v3'
```

To try it on synthetic data first: `memex scan --path examples/sample-corpus --index`.

### 4. (Optional) wire it into your coding agent

```bash
memex install all             # registers the MCP server + hooks for Claude Code / Codex / Cursor
memex install uninstall       # reverse it anytime
```

Your agent can then call Memex's MCP tools mid-session (recall a similar error,
predict the next action, load a project memory primer). See the
[README](../README.md#mcp-server--agent-integration) for the tool list.

---

## Verify

```bash
codesign --verify --verbose=2 /Applications/Memex.app   # valid Developer ID signature
spctl --assess --type execute --verbose /Applications/Memex.app   # → accepted (notarized)
```

For the full first-run flow (ports, build variants, expected output), see the
[README Quick start](../README.md#quick-start).
