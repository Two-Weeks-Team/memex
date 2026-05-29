# syntax=docker/dockerfile:1.7
#
# Single self-contained image for the server-based Memex variant:
#   Qdrant + memex web service (UI + JSON API + HTTP MCP) in ONE container.
#
# Build:  docker build -t memex-allinone -f deploy/web/Dockerfile .
# Run:    docker run --rm -p 8765:8765 memex-allinone
#
# The web binary is built with `--no-default-features --features web`, so it
# links NO Tauri/WebKit and runs on a slim Debian runtime.

# ---- 1. Qdrant binary source ------------------------------------------------
FROM qdrant/qdrant:v1.18.1 AS qdrant

# ---- 2. Rust builder (web feature only, no Tauri) ---------------------------
# trixie (glibc 2.41 / GCC 14): the prebuilt static ONNX Runtime that ort-sys
# downloads references glibc 2.38+ symbols (__isoc23_*) and newer libstdc++
# (__cxa_call_terminate), so bookworm (glibc 2.36) fails to link.
FROM rust:1-trixie AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Only the bits the web build needs (no tauri.conf.json / icons / capabilities —
# build.rs skips tauri_build when the `gui` feature is off).
COPY src-tauri/Cargo.toml src-tauri/Cargo.lock ./src-tauri/
COPY src-tauri/build.rs ./src-tauri/build.rs
COPY src-tauri/src ./src-tauri/src
# Bench source is required even for `cargo build --features web` because the
# `[[bench]] name = "quant_sweep"` entry in Cargo.toml makes cargo verify the
# path during manifest parsing — PR #18 added the bench, but this COPY was
# missed, so any `docker build` after that PR fails with "can't find
# `quant_sweep` bench at `benches/quant_sweep.rs`". Bench code is not actually
# compiled into the runtime image; the copy is ~50KB and avoids a manifest
# error before the build can start.
COPY src-tauri/benches ./src-tauri/benches
COPY src-tauri/fixtures ./src-tauri/fixtures

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/src-tauri/target \
    set -eux; \
    cargo build --release --no-default-features --features web --manifest-path src-tauri/Cargo.toml; \
    mkdir -p /out; \
    cp src-tauri/target/release/memex /out/memex; \
    # Copy the ONNX Runtime shared lib (fastembed/ort) if it was produced as a
    # dynamic lib; harmless no-op if ort linked it statically.
    find src-tauri/target/release -maxdepth 4 -name 'libonnxruntime*.so*' -exec cp -v {} /out/ \; || true; \
    ls -l /out

# ---- 3. Runtime (slim, no Tauri/WebKit) ------------------------------------
# Must match the builder's glibc/libstdc++ (trixie) for the ONNX Runtime symbols.
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl libgomp1 libunwind8 tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --shell /usr/sbin/nologin memex

# memex binary + any onnxruntime .so live together; PATH + LD_LIBRARY_PATH point here.
COPY --from=builder /out /opt/memex
COPY --from=qdrant  /qdrant/qdrant /usr/local/bin/qdrant
# Static UI. The synthetic corpus lives UNDER $HOME/.claude/projects so that
# list_sessions, replay, and predict — which re-parse source jsonl through the
# `sec` sandbox (trusted roots: ~/.claude/projects, ~/.codex/sessions) — work.
COPY src /app/ui
COPY examples/sample-corpus /home/memex/.claude/projects/memex-sample
COPY deploy/web/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh /opt/memex/memex /usr/local/bin/qdrant

ENV PATH="/opt/memex:${PATH}" \
    LD_LIBRARY_PATH="/opt/memex" \
    HOME="/home/memex" \
    XDG_CACHE_HOME="/home/memex/.cache" \
    MEMEX_QDRANT_URL="http://localhost:6334" \
    MEMEX_WEB_PORT="8765" \
    MEMEX_WEB_UI_DIR="/app/ui" \
    MEMEX_SCAN_ROOT="/home/memex/.claude/projects/memex-sample" \
    MEMEX_WEB_AUTOINDEX="1" \
    QDRANT__STORAGE__STORAGE_PATH="/home/memex/qdrant-storage"

# Pre-bake the embedding model into the image (no first-query network needed),
# create the Qdrant storage dir, and hand everything to the non-root user.
RUN memex warm-embedder \
 && mkdir -p /home/memex/qdrant-storage \
 && chown -R memex:memex /home/memex

WORKDIR /home/memex
USER memex

# 8765 = web (UI/API/MCP). 6333/6334 = Qdrant (exposed for inspection/health).
EXPOSE 8765 6333 6334
# tini as PID 1 → proper signal handling + zombie reaping for the two processes.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
