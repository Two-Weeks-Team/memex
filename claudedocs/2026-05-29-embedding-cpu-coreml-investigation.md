# Embedding CPU / GPU(CoreML) investigation & decision

**Date:** 2026-05-29
**Trigger:** Activity Monitor showed `Memex` at ~456–754% CPU with 0% GPU during use; goal is to run well on an **M1 MacBook Air, 8 GB RAM**.
**Status:** Decision made. CoreML rejected. This document exists so we **never re-run this investigation** — if the question comes back, read this first.

---

## TL;DR

- **High CPU during embedding is expected behavior, not a bug.** `fastembed` runs BGE‑small on the **ONNX Runtime CPU** backend and **hardcodes the intra‑op thread count to `available_parallelism()`** (every logical core). 0% GPU is normal for a CPU‑inference process (webview GPU work shows up under `WindowServer`).
- The pain is a **one‑time corpus warm‑up**: the v3 collection was nearly empty, so the watcher was embedding the whole backlog. Once indexed, the watcher is incremental (changed files only, capped per tick) and near‑idle.
- **CoreML (GPU/ANE) was measured and REJECTED for the 8 GB target**: it registers and offloads CPU, but used **~26.8 GB resident memory** + a multi‑second first‑run model‑compile. Fine to *measure* on a 32 GB+ machine; a non‑starter on an 8 GB Air.
- **QoS‑to‑E‑cores does NOT work** for this: raw `pthread_create` threads (which ONNX uses) do **not inherit** the creating thread's QoS on macOS.
- **Chosen path (minimum risk, time unlimited):** keep BGE‑small on CPU, bound Qdrant resources, make the warm‑up batch env‑tunable, and pursue the *proper* thread cap **upstream in fastembed** (no fork in our build). See `2026-05-29-embedding-cpu-coreml-investigation.md` companion PRs.

---

## Root cause (verified at source level)

`fastembed 5.13.4` (pinned in `src-tauri/Cargo.lock`), `src/text_embedding/impl.rs`:

```rust
use std::thread::available_parallelism;
let threads = available_parallelism()?.get();   // = logical core count (M1 = 8)
let mut builder = Session::builder()?
    .with_execution_providers(execution_providers)
    .with_optimization_level(GraphOptimizationLevel::Level3)
    .with_intra_threads(threads);                // ONNX intra-op = all cores, hardcoded
```

The **same hardcoding exists in the user‑defined and sparse/rerank paths** — there is no `InitOptions` method or env var to override it (verified against fastembed `main`/v5.14.0 `src/init.rs`).

**Consequence:** `main.rs` setting `OMP_NUM_THREADS` / `ORT_INTRA_OP_NUM_THREADS` / `ORT_NUM_THREADS` is a **no‑op** — fastembed never reads them; it passes `available_parallelism()` straight to ONNX. (The `watcher.rs` `MAX_INDEX_PER_TICK` comment documenting "~700% CPU" was written *after* that env block, confirming it never worked.)

---

## Verified facts (official sources)

| # | Fact | Source |
|---|------|--------|
| V1 | ONNX Runtime thread control = `intra_op_num_threads` via SessionOptions only. **No env var**; `OMP_NUM_THREADS` works only in an OpenMP build (removed from default ORT since v1.6, so inert for `ort`'s prebuilt binaries). Default intra‑op = #physical cores. | onnxruntime.ai/docs/performance/tune-performance/threading.html ; github.com/microsoft/onnxruntime/issues/14067 |
| V2 | fastembed 5.13.4 / 5.14.0 hardcodes `available_parallelism()` in **all** embedding paths; `InitOptions` exposes only `with_max_length`, `with_cache_dir`, `with_execution_providers`, `with_show_download_progress` — **no thread knob**. | Pinned source + github.com/Anush008/fastembed-rs/blob/main/src/init.rs |
| V3 | `std::thread::available_parallelism()` honors cgroup CPU quota / affinity **on Linux** (so Docker `--cpus=N` lowers it). **On macOS it returns the logical CPU count with no quota mechanism** (no cgroups; affinity is advisory). | doc.rust-lang.org/stable/std/thread/fn.available_parallelism.html |
| V4 | Qdrant supports Docker `mem_limit`/`cpus` and `optimizer_cpu_budget` to bound resources; quantization (scalar/int8, `always_ram`) cuts RAM ~75%. | qdrant.tech/documentation/faq/database-optimization/ ; qdrant.tech/articles/indexing-optimization/ |
| Q1 | macOS QoS **does** influence P‑core vs E‑core placement on Apple Silicon… | developer.apple.com/news/?id=vk3m204o (WWDC20 10686) |
| Q2 | …BUT threads created via `pthread_create` **do not inherit** the creator's QoS (default attr = `THREAD_QOS_LEGACY`; Apple: a thread "will not infer a QoS based on the context of its execution"). ONNX makes its intra‑op pool with raw pthreads → **setting QoS on the init thread does not reach the pool.** | Apple Energy Efficiency Guide (PrioritizeWorkWithQoS); github.com/apple/darwin-libpthread/blob/main/src/pthread.c |

**Why fastembed has no thread knob:** it's an **unaddressed gap, not a design choice.** No feature‑request issue ever existed; PR #122 ("support limit thread nums") *implemented exactly this* but the author self‑closed it in 28 min (bundled with an unstable `ort` bump), never reviewed/rejected. No maintainer rationale anywhere. (github.com/Anush008/fastembed-rs/pull/122)

---

## CoreML measurement (why it's rejected for 8 GB)

Spike: added `ort = { features=["coreml"] }` (macOS‑only) + `InitOptions::with_execution_providers([CoreML::default().build().error_on_failure()])` behind `MEMEX_EMBED_COREML=1`, then ran `memex reindex` on this M1 Max.

| Metric | CPU baseline (BGE‑small) | CoreML EP |
|---|---|---|
| EP registration | n/a | ✅ no `.error_on_failure()` error — **genuinely active, not silent fallback** |
| Avg cores (user+sys / real) | ~8–10 (pegs cores) | **≈1.44** (CPU offloaded) |
| **Max resident memory** | **0.44 GB** | **26.8 GB** ⚠️ |
| First‑run wall (7 sessions) | seconds | **53 s** (CoreML model compile dominates) |

**Verdict:** CoreML *works* and offloads CPU, but the **26.8 GB memory footprint + multi‑second compile** make it a non‑starter on an 8 GB Air (macOS ~3.5 GB + Memex + Qdrant must also fit → swap death). Matches official guidance that CoreML offers no reliable win for small, dynamic‑shape transformer embeddings and may regress. **Spike reverted.**

### Other options considered & why not (8 GB Air)
- **Switch to a GPU model (qwen3 / nomic‑v2‑moe via fastembed `metal` feature):** only those two models use the candle Metal backend; requires 384→768/1024 dim change → **full Qdrant schema migration + reindex + re‑tune** + larger model/RAM. Worse on 8 GB.
- **ort directly (bypass fastembed):** verified API but we'd reimplement tokenize/CLS‑pool/normalize → must match fastembed byte‑for‑byte or embeddings become incompatible (forced reindex). More code we own. Higher correctness risk than a 1‑line thread cap.
- **Vendored fork of fastembed:** correctness‑safe (1‑line `with_intra_threads`), but adds dependency/maintenance risk → rejected under "minimum risk."

---

## Decision (minimum risk, time unlimited)

Keep **BGE‑small on CPU** (0.44 GB, no reindex, no model swap, no CoreML, no fork in our build).

1. **Qdrant resource ceilings** — `docker-compose.yml`: `mem_limit: 4g`, `cpus: 4.0`. Pure config, reversible. `mem_limit` is a *ceiling, not a reservation* (Docker doesn't pre-allocate it); set generously at 4 GB so a healthy Qdrant is never OOM-killed, while quantization keeps actual usage <1 GB. Sized per Qdrant's official formula `num_vectors × dim × 4 × 1.5` (qdrant.tech capacity-planning), which their docs call "estimates at best — test in practice". (V4)
2. **Env‑tunable warm‑up batch** — `watcher.rs`: `MEMEX_WARMUP_BATCH` overrides `MAX_INDEX_PER_TICK` (default 30). Lower it on an Air → shorter CPU bursts; unlimited time absorbs the longer warm‑up.
3. **Proper thread cap → upstream** — submit a clean, standalone PR to fastembed‑rs adding `with_intra_threads` (default preserves `available_parallelism()`, opt‑in) against current `ort` rc.12 with tests. When merged, bump the version and set `intra_op = cores − 2`. First‑class, **zero fork debt**.

**Honest limit:** there is *no* zero‑code‑risk way to fully eliminate the CPU spike on macOS today (V1–V3 + Q2 close every in‑process door except a fastembed change). The plan shortens/bounds the spike now and gets the real cap via upstream later.

---

## Sources
- ONNX threading: https://onnxruntime.ai/docs/performance/tune-performance/threading.html
- ORT OpenMP removal: https://github.com/microsoft/onnxruntime/issues/14067
- fastembed init API / hardcode: https://github.com/Anush008/fastembed-rs/blob/main/src/text_embedding/impl.rs · https://github.com/Anush008/fastembed-rs/blob/main/src/init.rs
- fastembed PR #122 (the abandoned thread-config PR): https://github.com/Anush008/fastembed-rs/pull/122
- Rust available_parallelism: https://doc.rust-lang.org/stable/std/thread/fn.available_parallelism.html
- Apple QoS / P‑E cores: https://developer.apple.com/news/?id=vk3m204o · https://developer.apple.com/library/archive/documentation/Performance/Conceptual/EnergyGuide-iOS/PrioritizeWorkWithQoS.html
- Apple libpthread (no QoS inheritance): https://github.com/apple/darwin-libpthread/blob/main/src/pthread.c
- Qdrant optimization: https://qdrant.tech/documentation/faq/database-optimization/
