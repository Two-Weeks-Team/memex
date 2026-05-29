# Installing Memex on a clean machine

Memex ships as an **unsigned (ad-hoc signed) macOS app for Apple Silicon**.
This page is the honest, clean-machine install path — including exactly what
Gatekeeper will do and how to get past it safely, plus a **source-build
fallback** for judges who would rather not run an unsigned binary at all.

> No code signing, no Apple notarization. This is a hackathon MVP. If you
> prefer not to run unsigned binaries, jump to
> [Option B — build from source](#option-b--build-from-source) — it never
> downloads a binary.

---

## Prerequisites (runtime)

- macOS 11+ on **Apple Silicon (arm64)** — see [docs/BUILD.md](BUILD.md) for
  Intel/Linux notes.
- A local **Qdrant** on `localhost:6334`. One command:
  ```bash
  bash scripts/start-qdrant.sh
  ```
  Memex self-heals if you start Qdrant *after* launching the app.

---

## Option A — install the released DMG (unsigned)

```bash
# 1. Download Memex_0.1.0_aarch64.dmg from the release
open "https://github.com/Two-Weeks-Team/memex/releases/latest"

# 2. Open the .dmg and drag Memex.app to /Applications
```

### First launch: Gatekeeper will block it. Here's why, and the fix.

Memex is **ad-hoc signed only** — verifiable on the app you downloaded:

```console
$ codesign -dv /Applications/Memex.app
...
CodeDirectory v=20400 ... flags=0x20002(adhoc,linker-signed)
Signature=adhoc
TeamIdentifier=not set
```

`Signature=adhoc` + `TeamIdentifier=not set` means there is no Apple
Developer ID and no notarization ticket, so macOS quarantines it on download
and refuses a plain double-click the first time. Two safe ways past it:

**Fix 1 — right-click → Open (no Terminal, recommended):**
1. In `/Applications`, **right-click** (or Control-click) **Memex.app**.
2. Choose **Open**.
3. In the dialog, click **Open** again.

This records your consent for this specific app; afterwards it launches
normally. (A plain double-click on first launch only offers "Move to Trash" —
you must use right-click → Open.)

**Fix 2 — clear the quarantine flag (Terminal):**
```bash
xattr -dr com.apple.quarantine /Applications/Memex.app
```
This removes only the `com.apple.quarantine` extended attribute that macOS
adds to downloaded files — the flag that triggers the Gatekeeper prompt. It
does not disable Gatekeeper system-wide and does not modify the app's code.
After this, Memex opens with a normal double-click.

> Why this is required (and tested): the bundle is ad-hoc signed (evidence
> above). On macOS, a quarantined, non-notarized app cannot pass Gatekeeper
> without explicit user consent — `xattr -dr com.apple.quarantine` or
> right-click → Open are the two standard, documented ways to grant it.

### After first launch: grant Full Disk Access

Memex reads `~/.claude/projects` (and `~/.codex/sessions`) locally. On macOS
Sequoia/Tahoe you must grant **Full Disk Access** to `Memex.app` in
**System Settings → Privacy & Security → Full Disk Access**, then relaunch.
Memex never sends sessions anywhere — all parsing/embedding/search is local.

---

## Option B — build from source

For judges who would rather not run an unsigned binary, building from source
produces the **same binary, signed ad-hoc by your own machine**, and never
downloads a prebuilt artifact.

```bash
gh repo clone Two-Weeks-Team/memex ~/memex && cd ~/memex
npm install
bash scripts/start-qdrant.sh
cargo build --release --manifest-path src-tauri/Cargo.toml   # builds the CLI+GUI binary
npm run tauri build                                          # produces a local Memex.app
open src-tauri/target/release/bundle/macos/Memex.app
```

You can skip the GUI entirely and verify everything from the CLI — see
[docs/BUILD.md](BUILD.md) for the CLI smoke path and the build-only
verification path. Full step-by-step (prereqs, indexing) is in the
[README Quick start](../README.md#-quick-start).

---

## Verifying what you ran

```bash
codesign -dv /Applications/Memex.app          # → Signature=adhoc (expected)
spctl -a -vvv -t exec /Applications/Memex.app  # Gatekeeper assessment
```

A non-notarized app is expected to be *rejected* by `spctl` until you grant
consent via right-click → Open or clear quarantine — that is the whole reason
for the steps above.
