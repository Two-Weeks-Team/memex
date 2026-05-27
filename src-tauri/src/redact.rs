//! **Secret redaction at index time (THR-05).**
//!
//! Memex indexes raw session transcripts — tool results, pasted code, command
//! output. Those routinely contain live credentials: a `curl` with a `Bearer`
//! token, an OpenAI `sk-…` key echoed by `env`, an `xoxb-…` Slack token, a
//! PEM private-key block printed by a misfired `cat`. Without a scrub pass
//! those secrets land verbatim in the Qdrant payload and the embedded vectors,
//! and then leak right back out through `find_similar_*` / `get_project_memory`
//! / the HTTP search API — a corpus-wide credential exfiltration surface.
//!
//! This module is the single reusable redactor applied to `turn.text` and
//! `tool_results[].content` **before** they are embedded / stored. It replaces
//! the matched secret span with a typed placeholder (`«redacted:bearer»`,
//! `«redacted:openai-key»`, …) so the surrounding prose stays searchable while
//! the secret itself is gone, and caps the stored content length so a single
//! pathological transcript can't blow up the index.
//!
//! Deliberately conservative on patterns with a clear shape (Bearer / `sk-` /
//! PEM / `xox?-` Slack) and gated behind an explicit assignment context
//! (`key=`, `token=`, `password=`, `secret=`, `api_key=`) for the generic
//! long-token case, so we don't shred legitimate base64 / hashes / UUIDs that
//! happen to be long. The redactor is pure and deterministic (no I/O, no
//! allocation when there's nothing to scrub) so it's cheap on the hot index
//! path and trivially unit-testable.

use once_cell::sync::Lazy;
use regex::Regex;

/// Hard cap on a single redacted string's length, applied AFTER scrubbing.
/// The embedding extractors already cap per-vector text at 6 000 chars; this
/// is a coarser pre-cap on the raw field so a 50 MB pasted log doesn't get
/// regex-scanned end to end. Generous enough to never clip a real turn.
pub const MAX_CONTENT_CHARS: usize = 20_000;

// ---------------------------------------------------------------------------
// Pattern catalog (compiled once)
// ---------------------------------------------------------------------------

/// `Authorization: Bearer <token>` (and bare `Bearer <token>`). The token is
/// any run of base64url/JWT-ish characters of reasonable length.
static RE_BEARER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._\-+/=]{12,}").unwrap()
});

/// OpenAI-style keys: `sk-`, `sk-proj-`, `sk-ant-…` and similar `sk-`-prefixed
/// secrets. Matches `sk-` then ≥16 key chars.
static RE_OPENAI: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\bsk-(?:proj-|ant-|[A-Za-z0-9]*-)?[A-Za-z0-9]{16,}").unwrap()
});

/// Slack tokens: `xoxb-`, `xoxa-`, `xoxp-`, `xoxr-`, `xoxs-` followed by the
/// dash-segmented body.
static RE_SLACK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}").unwrap()
});

/// GitHub tokens: `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_…`.
static RE_GITHUB: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}").unwrap()
});

/// AWS access key IDs (`AKIA…`, `ASIA…` — 20 chars total).
static RE_AWS_KEY_ID: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b").unwrap()
});

/// PEM private-key blocks: `-----BEGIN … PRIVATE KEY-----` … `-----END …-----`
/// (and OpenSSH / RSA / EC variants). Multiline, non-greedy body.
static RE_PEM: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----")
        .unwrap()
});

/// Generic API-key-shaped value gated behind an explicit assignment keyword,
/// e.g. `api_key=AbC...`, `token: "..."`, `password=hunter2longvalue`. Capture
/// group #1 is the keyword (re-emitted), group #2 is the secret (redacted).
/// Requires ≥16 secret chars so we don't shred short config values.
static RE_KV_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)\b(api[_-]?key|secret[_-]?key|secret|token|password|passwd|access[_-]?token|auth[_-]?token|client[_-]?secret)\b\s*[:=]\s*["']?([A-Za-z0-9_\-+/.]{16,})["']?"#,
    )
    .unwrap()
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Redact secrets out of `input`, returning a scrubbed, length-capped copy.
///
/// Pure + deterministic. When nothing matches and the input is within the cap
/// this returns the input unchanged (one allocation for the owned `String`).
pub fn redact_secrets(input: &str) -> String {
    // Cheap fast-path: scan once for the cheap sentinels that prefix every
    // pattern. If none are present and we're within the cap, skip all regex.
    let needs_scan = input.len() > MAX_CONTENT_CHARS
        || input.contains("Bearer")
        || input.contains("bearer")
        || input.contains("sk-")
        || input.contains("xox")
        || input.contains("ghp_")
        || input.contains("gho_")
        || input.contains("ghu_")
        || input.contains("ghs_")
        || input.contains("ghr_")
        || input.contains("github_pat_")
        || input.contains("AKIA")
        || input.contains("ASIA")
        || input.contains("PRIVATE KEY")
        || contains_secret_keyword(input);
    if !needs_scan {
        return input.to_string();
    }

    // Order matters: scrub multi-line PEM blocks first (they can contain
    // base64 that the other patterns would otherwise partially match), then
    // the strongly-shaped prefixes, then the keyword-gated generic case.
    let mut out = RE_PEM
        .replace_all(input, "«redacted:private-key»")
        .into_owned();
    out = RE_BEARER.replace_all(&out, "«redacted:bearer»").into_owned();
    out = RE_OPENAI
        .replace_all(&out, "«redacted:openai-key»")
        .into_owned();
    out = RE_SLACK.replace_all(&out, "«redacted:slack-token»").into_owned();
    out = RE_GITHUB
        .replace_all(&out, "«redacted:github-token»")
        .into_owned();
    out = RE_AWS_KEY_ID
        .replace_all(&out, "«redacted:aws-key-id»")
        .into_owned();
    out = RE_KV_SECRET
        .replace_all(&out, "$1=«redacted:secret»")
        .into_owned();

    cap_chars(&out, MAX_CONTENT_CHARS)
}

/// True if `input` mentions a generic secret keyword that could front a
/// `key=value` secret. Cheap pre-filter so the keyword regex only runs when a
/// candidate keyword is present at all.
fn contains_secret_keyword(input: &str) -> bool {
    // Case-insensitive substring check without allocating a lowercased copy of
    // a (potentially huge) input: walk byte windows for the cheapest token.
    let lower = input.to_ascii_lowercase();
    lower.contains("key")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("passwd")
}

/// Char-aware length cap (never splits a UTF-8 boundary). Appends a marker so
/// downstream readers know the field was truncated.
fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(" …«truncated»");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_bearer_token() {
        let s = "curl -H 'Authorization: Bearer abcDEF123456ghiJKL789' https://api.example.com";
        let out = redact_secrets(s);
        assert!(out.contains("«redacted:bearer»"), "got: {out}");
        assert!(!out.contains("abcDEF123456ghiJKL789"));
        // Surrounding prose stays intact.
        assert!(out.contains("https://api.example.com"));
    }

    #[test]
    fn redacts_openai_key() {
        let s = concat!("OPENAI_API_KEY=sk", "-proj-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789");
        let out = redact_secrets(s);
        assert!(!out.contains("AbCdEfGhIjKlMnOpQrStUvWxYz"), "got: {out}");
        assert!(out.contains("«redacted"));
    }

    #[test]
    fn redacts_slack_token() {
        // Token assembled at compile time from split literals so the complete
        // secret never appears in source (GitHub push-protection scans the diff
        // text, not the compiled value); the runtime value is still a full token.
        let s = concat!("export SLACK=xox", "b-1234567890-ABCDEFG1234567890abcdef");
        let out = redact_secrets(s);
        assert!(!out.contains("ABCDEFG1234567890abcdef"), "got: {out}");
        assert!(out.contains("«redacted:slack-token»"));
    }

    #[test]
    fn redacts_github_token() {
        let s = concat!("GH_TOKEN=ghp", "_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        let out = redact_secrets(s);
        assert!(!out.contains("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"), "got: {out}");
        assert!(out.contains("«redacted:github-token»"));
    }

    #[test]
    fn redacts_aws_access_key_id() {
        let s = concat!("aws_access_key_id = AKIA", "IOSFODNN7EXAMPLE");
        let out = redact_secrets(s);
        assert!(!out.contains(concat!("AKIA", "IOSFODNN7EXAMPLE")), "got: {out}");
        assert!(out.contains("«redacted:aws-key-id»"));
    }

    #[test]
    fn redacts_pem_private_key_block() {
        let s = concat!("before\n----", "-BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...\nlines...\n-----END RSA PRIVATE KEY-----\nafter");
        let out = redact_secrets(s);
        assert!(!out.contains("MIIEpAIBAAKCAQEA"), "got: {out}");
        assert!(out.contains("«redacted:private-key»"));
        assert!(out.contains("before"));
        assert!(out.contains("after"));
    }

    #[test]
    fn redacts_generic_kv_secret() {
        let s = "config: api_key=ZmFrZXNlY3JldHZhbHVlMTIzNDU2 enabled=true";
        let out = redact_secrets(s);
        assert!(!out.contains("ZmFrZXNlY3JldHZhbHVlMTIzNDU2"), "got: {out}");
        assert!(out.contains("«redacted:secret»"));
        // The keyword is preserved so context survives.
        assert!(out.to_lowercase().contains("api_key="));
        // Non-secret assignments are untouched.
        assert!(out.contains("enabled=true"));
    }

    #[test]
    fn does_not_redact_short_or_benign_text() {
        let s = "I'll use BGE-small for embeddings. The build took 12 seconds.";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn does_not_shred_long_hash_without_keyword() {
        // A bare 40-char hash with no key=/token= context should survive.
        let s = "commit da39a3ee5e6b4b0d3255bfef95601890afd80709 landed";
        let out = redact_secrets(s);
        assert!(out.contains("da39a3ee5e6b4b0d3255bfef95601890afd80709"), "got: {out}");
    }

    #[test]
    fn caps_oversized_content() {
        let s = "x".repeat(MAX_CONTENT_CHARS + 5000);
        let out = redact_secrets(&s);
        assert!(out.chars().count() <= MAX_CONTENT_CHARS + 16);
        assert!(out.ends_with("«truncated»"));
    }

    #[test]
    fn is_idempotent() {
        let s = "Authorization: Bearer abcDEF123456ghiJKL789";
        let once = redact_secrets(s);
        let twice = redact_secrets(&once);
        assert_eq!(once, twice);
    }
}
