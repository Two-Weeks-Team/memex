//! Late-interaction token-level embeddings (KB-01).
//!
//! BGE-small is a single-vector sentence encoder, not a token-level encoder. The
//! spec for KB-01 picks **option B**: split the input into 32-token sliding
//! windows (stride 8) and embed each chunk independently. The result is a
//! `Vec<Vec<f32>>` (`Vec<DenseVector>`), which we then upsert into the
//! `content_late` multi-vector slot. Qdrant uses MaxSim at query time to score
//! token-level similarity against the query chunks.
//!
//! Cap: 64 chunks per session — anything beyond ~1.5k tokens of content is
//! truncated. This bounds the per-session multivector cost (worst case 64 ×
//! 384 floats = 96 KB raw, compressed by TurboQuant) and keeps embed latency
//! bounded for very long sessions.
//!
//! NOTE on tokenization: BGE-small uses a WordPiece tokenizer, but we don't
//! have direct token-level access through fastembed-rs. We approximate via
//! whitespace splitting — this overshoots real BPE tokens by ~1.3× for
//! English prose, which is close enough that the 32-token window ≈ 24 BPE
//! tokens (well under the 512-token model max). For non-English / heavy
//! punctuation text the windows shrink in real-token terms; still safe.

use anyhow::Result;

use crate::indexer::Embedder;

/// Window size in (whitespace) tokens.
pub const CHUNK_TOKENS: usize = 32;
/// Stride between window starts in tokens — `CHUNK_TOKENS - STRIDE_TOKENS`
/// tokens of overlap so the boundaries don't fragment a phrase.
pub const STRIDE_TOKENS: usize = 24; // 32 - 8 overlap
/// Hard cap on how many chunks we'll embed per session.
pub const MAX_CHUNKS: usize = 64;

/// Split `text` into 32-token sliding windows (stride 8) and embed each as a
/// 384-d dense vector. Returns up to `MAX_CHUNKS` vectors. Empty input → empty
/// vec.
///
/// **Determinism**: fastembed-rs is deterministic for the same input + same
/// model, so calling this twice with the same text returns identical output
/// (asserted by `t_embed_token_level_deterministic`).
pub fn embed_token_level(embedder: &Embedder, text: &str) -> Result<Vec<Vec<f32>>> {
    let chunks = chunk_text(text);
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    embedder.embed(chunks)
}

/// Split `text` into overlapping 32-token windows. Public so the upsert path
/// can pre-count chunks before deciding whether to call `embed_token_level`.
pub fn chunk_text(text: &str) -> Vec<String> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    if tokens.len() <= CHUNK_TOKENS {
        // Single chunk — no need to slide.
        return vec![tokens.join(" ")];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut start = 0usize;
    while start < tokens.len() && chunks.len() < MAX_CHUNKS {
        let end = (start + CHUNK_TOKENS).min(tokens.len());
        let window = &tokens[start..end];
        if !window.is_empty() {
            chunks.push(window.join(" "));
        }
        if end == tokens.len() {
            break;
        }
        start += STRIDE_TOKENS;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_embedder() -> Option<Embedder> {
        // Tests that need the real model call this. If the fastembed cache is
        // missing AND we have no network, embed() will fail — those tests are
        // best-effort.
        Embedder::new().ok()
    }

    #[test]
    fn t_chunk_text_short_input_one_chunk() {
        let text = "hello world this is a short sample";
        let chunks = chunk_text(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn t_chunk_text_empty_input_zero_chunks() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   \n  ").is_empty());
    }

    #[test]
    fn t_chunk_text_long_input_sliding() {
        // 100 tokens → expect multiple chunks with stride 24.
        let words: Vec<String> = (0..100).map(|i| format!("w{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_text(&text);
        // First chunk should start with w0, span 32 tokens.
        assert!(chunks[0].starts_with("w0 w1"));
        let first_tokens: Vec<&str> = chunks[0].split_whitespace().collect();
        assert_eq!(first_tokens.len(), CHUNK_TOKENS);
        // Second chunk starts at w24 (stride = 24).
        assert!(chunks[1].starts_with("w24 w25"));
        // We expect more than one chunk and fewer than MAX_CHUNKS.
        assert!(chunks.len() > 1);
        assert!(chunks.len() <= MAX_CHUNKS);
    }

    #[test]
    fn t_embed_token_level_chunk_cap() {
        // 10_000-token text → ≤ MAX_CHUNKS (64).
        let words: Vec<String> = (0..10_000).map(|i| format!("t{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_text(&text);
        assert!(chunks.len() <= MAX_CHUNKS, "got {} chunks", chunks.len());
        assert_eq!(chunks.len(), MAX_CHUNKS);
    }

    // ---- The following tests need the real fastembed model. They will be
    // skipped if Embedder::new() fails (no network + no cached model). They
    // are NOT `#[ignore]` because in the developer environment + CI with
    // network the model downloads on first test run and is cached locally
    // — only truly offline isolated CI sees them skip.

    #[test]
    fn t_embed_token_level_returns_list() {
        let Some(e) = make_embedder() else {
            eprintln!("skip: no fastembed model available");
            return;
        };
        let out = embed_token_level(&e, "a short sample text").expect("embed");
        assert!(!out.is_empty());
    }

    #[test]
    fn t_embed_token_level_dim_correct() {
        let Some(e) = make_embedder() else {
            eprintln!("skip: no fastembed model available");
            return;
        };
        let out = embed_token_level(&e, "another sample for dim").expect("embed");
        for v in &out {
            assert_eq!(v.len(), 384, "BGE-small-en-v1.5 must emit 384-d");
        }
    }

    #[test]
    fn t_embed_token_level_deterministic() {
        let Some(e) = make_embedder() else {
            eprintln!("skip: no fastembed model available");
            return;
        };
        let text = "deterministic call must give same output";
        let a = embed_token_level(&e, text).expect("first");
        let b = embed_token_level(&e, text).expect("second");
        assert_eq!(a.len(), b.len());
        for (i, (va, vb)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(va.len(), vb.len(), "chunk {i} dim mismatch");
            for (j, (xa, xb)) in va.iter().zip(vb.iter()).enumerate() {
                let d = (xa - xb).abs();
                assert!(d < 1e-5, "chunk {i} dim {j}: |{xa}-{xb}|={d}");
            }
        }
    }
}
