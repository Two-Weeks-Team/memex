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
      ca-certificates curl libgomp1 libunwind8 \
    && rm -rf /var/lib/apt/lists/*

# memex binary + any onnxruntime .so live together; PATH + LD_LIBRARY_PATH point here.
COPY --from=builder /out /opt/memex
COPY --from=qdrant  /qdrant/qdrant /usr/local/bin/qdrant
# Static UI + a ready-to-load synthetic corpus.
COPY src /app/ui
COPY examples/sample-corpus /app/sample-corpus
COPY deploy/web/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh /opt/memex/memex

ENV PATH="/opt/memex:${PATH}" \
    LD_LIBRARY_PATH="/opt/memex" \
    XDG_CACHE_HOME="/app/cache" \
    MEMEX_QDRANT_URL="http://localhost:6334" \
    MEMEX_WEB_PORT="8765" \
    QDRANT__STORAGE__STORAGE_PATH="/qdrant/storage" \
    MEMEX_WEB_UI_DIR="/app/ui"

# 8765 = web (UI/API/MCP). 6333/6334 = Qdrant (exposed for inspection/health).
EXPOSE 8765 6333 6334
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
