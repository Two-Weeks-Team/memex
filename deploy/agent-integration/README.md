# Memex Agent Integration (no plugin)

Make Memex proactively useful to **Claude Code** and **Codex** without a Claude Code
plugin. Four mechanisms; all fully functional (not an MVP).

> **Status**: source/templates for PR #8. The `memex install …` command and the
> `memex … --hook <event>` output modes these files call are Rust surfaces that land
> with PR #8 **on top of** PR #6 (web/HTTP MCP engine) + PR #7 (Companion/Wrapped/Loop
> Breaker). Until that engine is present, every script here **fails open** (no-op).

## Mechanisms

| # | Mechanism | Proactive? | Cross-agent | Files |
|---|---|---|---|---|
| ① | Raw MCP registration (stdio + HTTP) | pull | Claude / Codex / Cursor | committed `/.mcp.json`; `codex/config.toml.snippet`; `cursor/mcp.json.snippet` |
| ② | Claude Code hooks (SessionStart / UserPromptSubmit / PostToolUse / SessionEnd) | **yes (agent)** | Claude Code | `hooks/*.sh` + `settings.local.json.template` |
| ③ | Codex MCP + notify + AGENTS.md | yes (weak) | Codex | `codex/*` |
| ④ | Shell primer on `cd` | yes (human) | any shell | `shell/memex.{zsh,bash,fish,ps1}` |

## Why hooks are NOT committed (security)

`/.mcp.json` **is** committed (MCP also prompts before use; the stdio path carries no
credentials). The **hooks are not** — a committed `.claude/settings.json` that shells
out runs **arbitrary code on clone**, and `claude -p` (non-interactive) **disables the
trust prompt** entirely (THR-01, `claudedocs/reports/pr8/security-threat-model.md`).
So `memex install --hooks` writes them into your **gitignored** `.claude/settings.local.json`
— opt-in, per-developer, reversible (`memex install uninstall`).

## Install (lands with PR #8)

```bash
memex install all                 # MCP + hooks (settings.local.json) + shell + codex
memex install claude --hooks      # just Claude Code MCP + local hooks
memex install codex               # ~/.codex/config.toml + AGENTS.md
memex install cursor              # .cursor/mcp.json (project) or ~/.cursor/mcp.json
memex install shell               # append shell snippet to your rc
memex install uninstall           # remove every Memex-tagged block (idempotent)
```

Install is idempotent: JSON files are structurally merged and Memex's own groups are
tagged with a `MEMEX_HOOK=<id>` sentinel (and `# >>> memex >>>` fences for line-based
files), so re-running converges and your other hooks are never touched. Every write is
backed up (timestamped) and recorded in an `install-manifest.json`.

## Transport

`/.mcp.json` defaults to **stdio** (`command: "memex" mcp`) — zero bootstrap, the agent
spawns the process. For a shared/warm engine, switch to the HTTP profile against the
all-in-one container (`type: "http", url: "http://localhost:8765/mcp"`, see PR #6
`deploy/web/`). Hooks call the `memex` CLI (transport-agnostic) and **fail open** if no
engine is reachable.

## Files

```
/.mcp.json                          committed — ① raw MCP (stdio)
deploy/agent-integration/
  hooks/{session-start,user-prompt-submit,post-tool-use,session-end}.sh
  settings.local.json.template      what `install --hooks` merges (local, gitignored)
  shell/memex.{zsh,bash,fish,ps1}   ④ shell primer
  codex/{config.toml.snippet,AGENTS.md.snippet}
  cursor/mcp.json.snippet           ① Cursor MCP
docs/agent-integration.md           user guide
```
