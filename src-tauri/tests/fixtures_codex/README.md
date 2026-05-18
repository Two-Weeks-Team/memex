# Codex fixtures

Synthesized minimal Codex rollout JSONL files used by the parser unit tests in
`src-tauri/src/codex_parser.rs` and the integration tests in
`src-tauri/tests/codex_parser_integration.rs`.

These are NOT copies of real `~/.codex/sessions/` data — they are hand-crafted
to exercise specific parser code paths while staying small and sanitized.
Field shapes match the real Codex CLI rollout schema verified against
`~/.codex/sessions/2026/05/18/rollout-2026-05-18T16-04-23-019e39e6-*.jsonl`.

## Fixtures

Located at `src-tauri/tests/fixtures_codex/` (kept separate from
`fixtures/` so the Claude `parser::scan_dir` tests aren't polluted by
Codex-formatted files).

All fixtures use the real `rollout-*.jsonl` filename prefix the Codex CLI
emits — `scan_codex_dir` filters on this prefix so fixture rename matters.

| File | Purpose |
| ---- | ------- |
| `rollout-01-minimal.jsonl` | session_meta + 1 user + 1 assistant message — the absolute minimum a valid Codex session can carry. |
| `rollout-02-with-tools.jsonl` | Adds 2 function_call + 2 function_call_output pairs to exercise tool wiring. |
| `rollout-03-with-errors.jsonl` | function_call_output containing error markers (`stderr:`, `Error:`) so `has_errors` flips true. |
| `rollout-04-long-session.jsonl` | ~50 turns + mixed tool calls — stress-tests counts/aggregation. |
| `rollout-05-empty-after-meta.jsonl` | Only the session_meta line, no turns — edge case for "did we crash?" |

## Notes

- Codex rollouts always start with a `session_meta` line — every fixture
  follows this invariant.
- The `arguments` field of `function_call` is a JSON-encoded **string**, not
  a JSON object (verified from real session data). The parser must
  `serde_json::from_str` it back into a `Value`.
- The `cwd` field of `session_meta.payload` is already absolute. Do NOT
  apply the Claude-style `-Users-x-foo` encoded-cwd decode.
- Fixtures intentionally use the same `timestamp` format the real Codex CLI
  emits (RFC 3339 with `Z` suffix).
