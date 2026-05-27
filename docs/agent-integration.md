# Agent Integration — make Memex proactive in Claude Code & Codex

Memex indexes your past coding sessions. This guide wires it so an agent (Claude Code,
Codex) **automatically** reaches that memory at the right moments — without a plugin.

There are two postures, and you want both:

- **Pull** — the agent calls a Memex MCP tool when it decides to. Universal, low-risk.
- **Push** — Memex injects context automatically (proactive). Lives in host *hooks*,
  not in MCP (MCP tools cannot interrupt the agent's turn).

## Prerequisites

The Memex engine must be runnable: either the `memex` binary on `PATH` (stdio) or the
all-in-one container (`deploy/web/`) exposing HTTP MCP on `:8765`. Hooks **fail open** —
if the engine is down, the session proceeds with no memory rather than stalling.

## ① Universal MCP (pull) — every agent

A committed `/.mcp.json` registers the `memex` server (stdio). Claude Code prompts once
to trust the project, then exposes the Memex tools (`get_project_memory`,
`generate_wrapped_report`, search/recall/lens/topology/…).

- **Codex**: merge `deploy/agent-integration/codex/config.toml.snippet` into
  `~/.codex/config.toml` (or `memex install codex`).
- **Cursor**: a `.cursor/mcp.json` with the same server block (`memex install cursor` writes this for you).

Keep it pull-only? You're done. For proactivity, add ②–④.

## ② Claude Code hooks (push — into the agent's turn)

`memex install --hooks` writes four hooks into your **gitignored**
`.claude/settings.local.json` (not committed — see Security):

| Hook | Fires | What Memex does |
|---|---|---|
| **SessionStart** | session begins / resumes / clears | injects a Companion **primer** (past intents, decisions, pitfalls) as factual context, ~200–500 tokens |
| **UserPromptSubmit** | every prompt | if a past session is relevant enough, injects a short recall note (relevance-gated to avoid noise) |
| **PostToolUse** (Bash) | after a Bash tool call | on a repeated-error stuck pattern, injects "what past-you ran next" (Loop Breaker) |
| **SessionEnd** | session ends | reindexes the just-finished session so next time is fresh (detached, non-blocking) |

All four are fail-open and bounded (a slow/dead engine never blocks you).

## ③ Codex (push — weaker than Claude Code)

- MCP server + an `AGENTS.md` note telling Codex to call `get_project_memory` first.
- `notify = ["memex","codex-notify"]` surfaces a Loop Breaker pivot on repeated errors.
- Codex's `notify` output isn't injected into the model, so the primer relies on the
  MCP tool + AGENTS.md rather than automatic injection.

## ④ Shell primer (push — any terminal)

`source deploy/agent-integration/shell/memex.zsh` (or `.bash` / `.fish` / `.ps1`) prints
the project primer when you `cd` into a repo. Works for Codex / Cursor / a bare
terminal — it's human-facing (terminal), not injected into an agent's context.

## Security (read before committing config)

- Only `/.mcp.json` is committed. **Hooks are never committed** — a committed hook runs
  arbitrary code on clone, and `claude -p` disables the trust prompt. Hooks live in your
  local, gitignored settings via `memex install --hooks`.
- Memory comes from past session transcripts, which may contain secrets/PII. Secret
  redaction at index time, injected/printed-text sanitization, and a `127.0.0.1`-only
  Qdrant bind (+ API key) are **planned, not yet active** — they land with the Rust
  implementation after PR #6 and #7 merge (tracked as THR-05/THR-06 in
  `claudedocs/reports/pr8/security-threat-model.md`). Until then, run the engine on a
  trusted network and do **not** index sessions containing secrets.

## Uninstall

```bash
memex install uninstall      # removes every Memex-tagged block; idempotent; backed up
```
