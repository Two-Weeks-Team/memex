//! `memex` CLI mode. Activates when the binary is invoked with a recognized
//! subcommand. Otherwise main.rs falls through to the Tauri GUI.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::{codex_parser, companion, crud, indexer, parser};

#[derive(Debug, Parser)]
#[command(name = "memex", version, about = "Time Machine for AI session JSONL")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Walk session roots (Claude + Codex) and print a one-line summary per session.
    Scan {
        /// Path to scan. If unset, scans BOTH `~/.claude/projects` and `~/.codex/sessions`.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Filter by agent (P5 KH-01). `claude`, `codex`, or `all` (default).
        #[arg(long, default_value = "all")]
        agent: String,
        /// Also index parsed sessions into Qdrant (creates collection if needed).
        #[arg(long)]
        index: bool,
        /// Cap the number of sessions printed.
        #[arg(long)]
        limit: Option<usize>,
        /// THR-05: strip secrets (Bearer tokens, sk-… keys, PEM blocks,
        /// key=…-shaped values) from indexed text. ON by default; pass
        /// `--redact=false` only for redaction-fixture tests.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        redact: bool,
    },
    /// Vector search against the indexed `content` field.
    Search {
        /// Free-text query.
        query: String,
        /// Number of results.
        #[arg(long, default_value_t = 10)]
        limit: u64,
    },
    /// Weighted multi-vector lens search (Phase 3 T3.1).
    Lens {
        query: String,
        #[arg(long, default_value_t = 1.0)]
        content: f32,
        #[arg(long, default_value_t = 1.0)]
        tool: f32,
        #[arg(long, default_value_t = 1.0)]
        path: f32,
        #[arg(long, default_value_t = 1.0)]
        error: f32,
        #[arg(long, default_value_t = 1.0)]
        code: f32,
        #[arg(long, default_value_t = 10)]
        limit: u64,
    },
    /// Discovery API: find sessions like the positives, unlike the negatives (T3.2).
    Mix {
        /// Positive session ids (comma-sep or repeated).
        #[arg(long = "pos", value_delimiter = ',', num_args = 1..)]
        positive: Vec<String>,
        /// Negative session ids (comma-sep or repeated).
        #[arg(long = "neg", value_delimiter = ',', num_args = 0..)]
        negative: Vec<String>,
        #[arg(long, default_value_t = 10)]
        limit: u64,
    },
    /// Distance Matrix → MST topology of the collection (T3.3).
    Topology {
        /// How many sessions to sample.
        #[arg(long, default_value_t = 80)]
        sample: u32,
        /// Nearest neighbors per sampled point.
        #[arg(long, default_value_t = 5)]
        per_point: u32,
        /// If set, write the JSON output to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Proactive recall — search for past sessions that solved a similar error (T3.6).
    ///
    /// In hook mode (`--hook user-prompt-submit`) the UserPromptSubmit JSON is
    /// read on stdin and `.prompt` is used as the query, behind a relevance
    /// gate (only emits when a match clears the threshold). `error_text` is
    /// optional in that mode.
    Recall {
        /// Free-text error / query. Optional when `--hook user-prompt-submit`
        /// (the query then comes from the stdin prompt JSON).
        error_text: Option<String>,
        #[arg(long, default_value_t = 5)]
        limit: u64,
        /// Sandbox the recall to a working directory's project (reserved for
        /// the project-path filter; also used to resolve hook context).
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Emit Claude Code hook JSON for the given event instead of a table.
        /// Supported: `user-prompt-submit`.
        #[arg(long)]
        hook: Option<String>,
        /// Override the prompt text instead of reading it from stdin (testing).
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Predict next likely actions for a session by mining how similar past
    /// sessions proceeded from a comparable conversational position.
    Predict {
        /// Session ID to predict from (the "active" session).
        session_id: String,
        /// How many recent turns of the active session to use as context.
        #[arg(long, default_value_t = 3)]
        last_n: usize,
        /// How many turns ahead to walk in each neighbor.
        #[arg(long, default_value_t = 3)]
        horizon: usize,
        /// How many similar past sessions to consult.
        #[arg(long, default_value_t = 8)]
        neighbors: u64,
    },
    /// Snapshot management.
    Snapshot {
        #[command(subcommand)]
        op: SnapshotOp,
    },
    /// Start the Model Context Protocol server over stdio. Any MCP-aware
    /// agent (Claude Code, Codex, Cursor, …) can register Memex via:
    ///   claude mcp add memex /path/to/memex mcp
    Mcp,
    /// Print the `claude mcp add` command for this binary (and optionally run it).
    InstallMcp {
        /// Also execute the command instead of just printing.
        #[arg(long)]
        run: bool,
    },
    /// **Cold Start Killer.** Compose a memory primer for `cwd` from past
    /// sessions in the same codebase (or semantically similar projects).
    /// The markdown output is the same one the MCP `get_project_memory`
    /// tool returns — pipe it to `pbcopy` and paste into a new session as
    /// a system message:
    ///   memex memory --cwd "$(pwd)" | pbcopy
    Memory {
        /// Directory to compose for. Defaults to the current process cwd.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Max past sessions to mine.
        #[arg(long, default_value_t = 8)]
        limit: usize,
        /// Emit the full structured JSON instead of just the markdown.
        #[arg(long)]
        json: bool,
        /// Emit Claude Code hook JSON for the given event instead of plain
        /// markdown. Supported: `session-start`, `shell`.
        #[arg(long)]
        hook: Option<String>,
    },
    /// **Engineering Wrapped.** A monthly / weekly digest of your corpus:
    /// top tools, top binaries, intent / arc / outcome mix, repeated
    /// decisions, debugging fingerprint. Designed to be screenshot-shared.
    ///   memex wrapped --window-days 30 | pbcopy
    Wrapped {
        /// Time window in days. 0 = all-time.
        #[arg(long, default_value_t = 30)]
        window_days: u32,
        /// Max sessions to deep-mine for repeated-decision detection.
        #[arg(long, default_value_t = 32)]
        limit: usize,
        /// Emit JSON instead of the markdown digest.
        #[arg(long)]
        json: bool,
    },
    /// **Agent integration installer.** Wire Memex into Claude Code / Codex /
    /// Cursor / your shell without a plugin. Structural JSON merge into
    /// settings.local.json (idempotent via the `MEMEX_HOOK=` sentinel) +
    /// fenced `# >>> memex >>>` markers for line-based files; atomic writes
    /// with timestamped backups.
    ///   memex install all
    ///   memex install claude --hooks
    ///   memex install uninstall
    Install {
        /// One of: `claude`, `codex`, `cursor`, `shell`, `all`, `uninstall`.
        target: String,
        /// `user` (default — home dir) or `project` (./.claude, ./.cursor, …).
        #[arg(long, default_value = "user")]
        scope: String,
        /// MCP transport to register: `stdio` (default, zero-bootstrap) or
        /// `http` (shared warm engine on :8765).
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// Also install Claude Code local hooks (SessionStart / UserPromptSubmit
        /// / PostToolUse / SessionEnd) into settings.local.json.
        #[arg(long)]
        hooks: bool,
        /// Resolve + report what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Override refuse-and-warn guards (e.g. clobber an existing Codex
        /// `notify` program).
        #[arg(long)]
        force: bool,
    },
    /// **Incremental reindex** for a working directory (SessionEnd hook +
    /// manual use). Self-debounces (skips if the same cwd was reindexed within
    /// a short window) and is idempotent. Detached-friendly: returns fast.
    Reindex {
        /// Directory whose project sessions to refresh. Defaults to cwd.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Specific session jsonl to reindex (overrides the cwd scan).
        #[arg(long)]
        session: Option<PathBuf>,
        /// Hook event name (informational; reindex emits no context).
        #[arg(long)]
        hook: Option<String>,
        /// THR-05 secret redaction (ON by default).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        redact: bool,
    },
    /// **Loop Breaker (headless).** Reads a Claude Code PostToolUse hook JSON
    /// on stdin (tool_name / tool_input / tool_response), detects the stuck
    /// pattern via the lifted LOOP_* thresholds, and emits a pivot via the
    /// `--hook` JSON envelope — or nothing (true-negative).
    LoopCheck {
        /// Working directory of the active session.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Hook event name (e.g. `post-tool-use`). Required for output shape.
        #[arg(long)]
        hook: String,
    },
    /// **Codex notify hook.** Parses Codex's turn-complete payload from
    /// `argv[1]` (NOT stdin); on a stuck signal, surfaces a Loop Breaker hint
    /// to stderr.
    CodexNotify {
        /// The JSON payload Codex passes as the first argument.
        payload: Option<String>,
    },
    /// Start the headless web service: serves the UI + JSON API + HTTP MCP at
    /// `/mcp`. Used by the single Docker image. (`web` feature only.)
    #[cfg(feature = "web")]
    Serve {
        /// Port to listen on.
        #[arg(long, default_value_t = 8765)]
        port: u16,
        /// Directory of static UI assets to serve.
        #[arg(long, default_value = "src")]
        ui_dir: PathBuf,
    },
    /// Download/load the embedding model into the cache, then exit. Used to
    /// pre-bake the model into the Docker image so first query needs no
    /// network. (`web` feature only.)
    #[cfg(feature = "web")]
    WarmEmbedder,
}

#[derive(Debug, Subcommand)]
pub enum SnapshotOp {
    /// Export a snapshot of the current collection to `path`.
    Export { path: PathBuf },
    /// Restore a collection from a snapshot file.
    Import { path: PathBuf },
}

pub fn run(args: Vec<String>) -> Result<()> {
    let cli = Cli::try_parse_from(args)?;
    match cli.command {
        Command::Scan { path, agent, index, limit, redact } => {
            cmd_scan(path, agent, index, limit, redact)
        }
        Command::Search { query, limit } => cmd_search(query, limit),
        Command::Lens {
            query,
            content,
            tool,
            path,
            error,
            code,
            limit,
        } => cmd_lens(query, content, tool, path, error, code, limit),
        Command::Mix {
            positive,
            negative,
            limit,
        } => cmd_mix(positive, negative, limit),
        Command::Topology {
            sample,
            per_point,
            out,
        } => cmd_topology(sample, per_point, out),
        Command::Recall {
            error_text,
            limit,
            cwd,
            hook,
            prompt,
        } => cmd_recall(error_text, limit, cwd, hook, prompt),
        Command::Predict {
            session_id,
            last_n,
            horizon,
            neighbors,
        } => cmd_predict(session_id, last_n, horizon, neighbors),
        Command::Snapshot { op } => cmd_snapshot(op),
        Command::Mcp => cmd_mcp(),
        Command::InstallMcp { run } => cmd_install_mcp(run),
        Command::Memory { cwd, limit, json, hook } => cmd_memory(cwd, limit, json, hook),
        Command::Wrapped { window_days, limit, json } => cmd_wrapped(window_days, limit, json),
        Command::Install {
            target,
            scope,
            transport,
            hooks,
            dry_run,
            force,
        } => cmd_install(target, scope, transport, hooks, dry_run, force),
        Command::Reindex {
            cwd,
            session,
            hook,
            redact,
        } => cmd_reindex(cwd, session, hook, redact),
        Command::LoopCheck { cwd, hook } => cmd_loop_check(cwd, hook),
        Command::CodexNotify { payload } => cmd_codex_notify(payload),
        #[cfg(feature = "web")]
        Command::Serve { port, ui_dir } => cmd_serve(port, ui_dir),
        #[cfg(feature = "web")]
        Command::WarmEmbedder => {
            let _ = indexer::Embedder::new()?;
            println!("embedder model ready");
            Ok(())
        }
    }
}

fn cmd_wrapped(window_days: u32, limit: usize, json: bool) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        // Wrapped is payload-only aggregation — no embedder needed.
        // (Codex P2-a: skipped the 130MB BGE-small first-run download.)
        let report =
            crate::wrapped::compose_wrapped(&client, window_days, limit).await?;
        eprintln!(
            "[memex wrapped] window={}d sessions={} mined={} tool_calls={} ({} ms)",
            window_days,
            report.sessions_total,
            report.sessions_mined,
            report.total_tool_calls,
            report.elapsed_ms,
        );
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", report.markdown);
        }
        anyhow::Ok(())
    })
}

fn cmd_install(
    target: String,
    scope: String,
    transport: String,
    hooks: bool,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    use crate::install::{InstallOptions, Scope, Transport};
    let scope = match scope.to_lowercase().as_str() {
        "user" => Scope::User,
        "project" => Scope::Project,
        other => anyhow::bail!("--scope must be 'user' or 'project', got {other:?}"),
    };
    let transport = match transport.to_lowercase().as_str() {
        "stdio" => Transport::Stdio,
        "http" => Transport::Http,
        other => anyhow::bail!("--transport must be 'stdio' or 'http', got {other:?}"),
    };
    let opts = InstallOptions {
        scope,
        transport,
        hooks,
        dry_run,
        force,
        ..InstallOptions::default()
    };
    crate::install::run(&target, &opts)?;
    if dry_run {
        eprintln!("[memex install] dry-run complete — no files were modified.");
    } else {
        eprintln!("[memex install] done.");
    }
    Ok(())
}

/// Debounce window for `memex reindex`: skip if the same cwd was reindexed
/// within this window. SessionEnd + the GUI watcher can both fire near the
/// same moment; this stops the thrash (PR8-PLAN risk #6).
const REINDEX_DEBOUNCE_SECS: u64 = 90;

fn cmd_reindex(
    cwd: Option<PathBuf>,
    session: Option<PathBuf>,
    hook: Option<String>,
    redact: bool,
) -> Result<()> {
    let _ = hook; // reindex injects no context; the flag is accepted for parity.
    indexer::set_redaction_enabled(redact);
    let resolved = companion::resolve_cwd_arg(cwd.as_deref())?;

    // Self-debounce via a per-cwd timestamp file under the cache dir. If we
    // reindexed this cwd within the window, return immediately (idempotent +
    // cheap for the SessionEnd hook which fires on every session close).
    if reindex_debounced(&resolved) {
        eprintln!("[memex reindex] debounced — {} reindexed recently", resolved.display());
        return Ok(());
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .context("building tokio runtime")?;
    rt.block_on(async {
        // Collect the sessions to (re)index: an explicit --session jsonl, else
        // every Claude/Codex session whose project_path matches the cwd.
        let sessions: Vec<parser::Session> = if let Some(sess_path) = session {
            // Validate the explicit session path through the sandbox.
            let validated = crate::sec::validate_session_path(&sess_path)
                .with_context(|| format!("validating --session {}", sess_path.display()))?;
            let parsed = if validated.to_string_lossy().contains("/.codex/sessions") {
                codex_parser::parse_codex_session(&validated)?
            } else {
                parser::parse_session(&validated)?
            };
            vec![parsed]
        } else {
            // Scan both roots, keep sessions whose project_path == cwd.
            let cwd_str = resolved.to_string_lossy().to_string();
            let mut all = scan_by_agent("all").unwrap_or_default();
            all.retain(|s| {
                s.project_path
                    .as_deref()
                    .map(|p| p == cwd_str)
                    .unwrap_or(false)
            });
            all
        };

        if sessions.is_empty() {
            eprintln!("[memex reindex] no sessions for {} — nothing to do", resolved.display());
            return anyhow::Ok(());
        }

        let client = indexer::connect().await?;
        crate::crud::ensure_collection_v3(&client)
            .await
            .context("ensuring v3 collection before reindex")?;
        indexer::ensure_collection(&client).await?;
        let embedder = std::sync::Arc::new(indexer::Embedder::new()?);
        let report = indexer::bulk_index_arc(&client, embedder, &sessions).await?;
        eprintln!(
            "[memex reindex] {} session(s) → indexed {} ({} dup, {} err) for {}",
            sessions.len(),
            report.indexed,
            report.duplicates_skipped,
            report.errors,
            resolved.display(),
        );
        anyhow::Ok(())
    })?;

    // Record the reindex time AFTER a successful pass.
    mark_reindexed(&resolved);
    Ok(())
}

/// Path of the per-cwd debounce timestamp file under the platform cache dir.
fn reindex_stamp_path(cwd: &std::path::Path) -> PathBuf {
    let mut dir = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push("memex");
    dir.push("reindex");
    // Hash the cwd into a stable filename (UUID v5) so arbitrary paths map to
    // a safe flat filename.
    let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, cwd.to_string_lossy().as_bytes());
    dir.push(format!("{id}.stamp"));
    dir
}

/// True if `cwd` was reindexed within `REINDEX_DEBOUNCE_SECS`.
fn reindex_debounced(cwd: &std::path::Path) -> bool {
    let stamp = reindex_stamp_path(cwd);
    let Ok(meta) = std::fs::metadata(&stamp) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|e| e.as_secs() < REINDEX_DEBOUNCE_SECS)
        .unwrap_or(false)
}

/// Touch the per-cwd debounce stamp file (best-effort; never fatal).
fn mark_reindexed(cwd: &std::path::Path) {
    let stamp = reindex_stamp_path(cwd);
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&stamp, b"");
}

fn cmd_loop_check(cwd: Option<PathBuf>, hook: String) -> Result<()> {
    let event = crate::hook::HookEvent::parse(&hook)
        .ok_or_else(|| anyhow::anyhow!("unknown --hook event: {hook}"))?;

    // Read the PostToolUse hook JSON (tool_name, tool_input, tool_response)
    // from stdin. Fail-open: any parse problem → emit nothing.
    let payload = {
        use std::io::Read;
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_err() {
            return Ok(());
        }
        match serde_json::from_str::<serde_json::Value>(buf.trim()) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        }
    };

    // Resolve the active session for `cwd` (most-recent session whose
    // project_path matches) and run the pure stuck-detection gate against it.
    let resolved = companion::resolve_cwd_arg(cwd.as_deref())?;
    let cwd_str = resolved.to_string_lossy().to_string();

    // Detect whether THIS PostToolUse delivered an error (tool_response carries
    // an error signal). When the tool succeeded the contract is a true-negative
    // — emit nothing — even if the session history is stuck, to avoid nagging
    // on a turn the agent is recovering on.
    if !tool_response_is_error(&payload) {
        return Ok(());
    }

    // Find the active session and check the lifted LOOP_* gate.
    let mut sessions = scan_by_agent("all").unwrap_or_default();
    sessions.retain(|s| {
        s.project_path
            .as_deref()
            .map(|p| p == cwd_str)
            .unwrap_or(false)
    });
    // Most-recent session is the active one.
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    let Some(active) = sessions.into_iter().next() else {
        return Ok(());
    };

    let Some(stuck) = crate::loopcheck::is_stuck(&active) else {
        return Ok(());
    };

    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("a tool");
    let body = format!(
        "# ⚠ Memex Loop Breaker\n\
You've hit {} tool error(s) in the last {} turns (latest: `{}`). You appear stuck \
on the same problem. Consider stepping back: re-read the error, change approach, \
or call the Memex `find_similar_error` / `predict_next_action` tools to see how a \
past session broke out of a comparable position.\n",
        stuck.recent_errors, stuck.recent_window, tool,
    );
    crate::hook::emit(event, &body);
    Ok(())
}

/// Heuristic: does a PostToolUse `tool_response` indicate an error? Claude Code
/// has no error-matcher event, so we sniff the response payload: an explicit
/// `is_error`/`error` flag, a non-zero `exit_code`/`returncode`, or
/// error-shaped text in a stringified response.
fn tool_response_is_error(payload: &serde_json::Value) -> bool {
    let Some(resp) = payload.get("tool_response") else {
        return false;
    };
    if let Some(b) = resp.get("is_error").and_then(|v| v.as_bool()) {
        if b {
            return true;
        }
    }
    if resp.get("error").map(|v| !v.is_null()).unwrap_or(false) {
        return true;
    }
    for k in ["exit_code", "returncode", "exitCode", "code"] {
        if let Some(n) = resp.get(k).and_then(|v| v.as_i64()) {
            if n != 0 {
                return true;
            }
        }
    }
    // Stringified response (Bash tool returns a string) — scan for error words.
    let text = match resp {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let lower = text.to_ascii_lowercase();
    lower.contains("error:")
        || lower.contains("traceback")
        || lower.contains("panic")
        || lower.contains("exception")
        || lower.contains("command failed")
        || lower.contains("fatal:")
}

fn cmd_codex_notify(payload: Option<String>) -> Result<()> {
    // Codex passes the turn-complete payload as argv[1] (NOT stdin).
    let Some(raw) = payload else {
        // No payload → nothing to do (fail-open).
        return Ok(());
    };
    let v: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    // Only react to turn-complete notifications.
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if kind != "agent-turn-complete" {
        return Ok(());
    }

    // Codex's payload doesn't carry the full session, so we look up the active
    // session for the process cwd and run the same stuck gate. On a stuck
    // signal, surface a Loop Breaker hint to STDERR (Codex notify stdout is not
    // model-injected; stderr is where the human sees it).
    let resolved = companion::resolve_cwd_arg(None)?;
    let cwd_str = resolved.to_string_lossy().to_string();
    let mut sessions = scan_by_agent("all").unwrap_or_default();
    sessions.retain(|s| {
        s.project_path
            .as_deref()
            .map(|p| p == cwd_str)
            .unwrap_or(false)
    });
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    if let Some(active) = sessions.into_iter().next() {
        if let Some(stuck) = crate::loopcheck::is_stuck(&active) {
            eprintln!(
                "[memex Loop Breaker] You've hit {} tool error(s) in the last {} turns — \
                 you appear stuck. Consider changing approach or calling the Memex \
                 `find_similar_error` MCP tool to see how a past session recovered.",
                stuck.recent_errors, stuck.recent_window,
            );
        }
    }
    Ok(())
}

fn cmd_memory(
    cwd: Option<PathBuf>,
    limit: usize,
    json: bool,
    hook: Option<String>,
) -> Result<()> {
    let resolved = companion::resolve_cwd_arg(cwd.as_deref())?;
    // Hook mode: parse + validate the event up front so an unknown event is a
    // clean error rather than silently producing plain markdown.
    let hook_event = match hook.as_deref() {
        Some(h) => Some(
            crate::hook::HookEvent::parse(h)
                .ok_or_else(|| anyhow::anyhow!("unknown --hook event: {h}"))?,
        ),
        None => None,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        // PR7-A lazy embedder: try a local-only primer first (offline / cold-
        // start safe). Only init the heavy embedder for the cross-project
        // semantic pass when no local project matched.
        let local = companion::compose_memory_primer_lazy(&client, None, &resolved, limit).await?;
        let primer = if local.matched_local_project {
            local
        } else {
            let embedder = indexer::Embedder::new()?;
            companion::compose_memory_primer_lazy(&client, Some(&embedder), &resolved, limit)
                .await?
        };
        eprintln!(
            "[memex memory] cwd={} sessions_used={}/{} decisions={} pitfalls={} ({} ms)",
            primer.cwd,
            primer.stats.neighbors_used,
            primer.stats.neighbors_searched,
            primer.stats.decisions_extracted,
            primer.stats.pitfalls_extracted,
            primer.elapsed_ms,
        );
        if let Some(event) = hook_event {
            // Emit the Claude Code hook JSON (or shell markdown) — sanitized,
            // empty → nothing. Only emit a primer that actually found sources;
            // a "no past sessions" primer is noise in a hook.
            if primer.source_sessions.is_empty() {
                return anyhow::Ok(());
            }
            crate::hook::emit(event, &primer.markdown);
        } else if json {
            println!("{}", serde_json::to_string_pretty(&primer)?);
        } else {
            print!("{}", primer.markdown);
        }
        anyhow::Ok(())
    })
}

#[cfg(feature = "web")]
fn cmd_serve(port: u16, ui_dir: PathBuf) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move { crate::web::serve(port, ui_dir).await })
}

fn cmd_mcp() -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        crate::mcp::run().await
    })
}

fn cmd_install_mcp(run: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let path = exe.canonicalize().unwrap_or(exe);
    println!("# Register Memex with Claude Code:");
    println!("claude mcp add memex {} mcp", path.display());
    println!();
    println!("# Then verify:");
    println!("claude mcp list   # should show 'memex ✓'");
    if run {
        let out = std::process::Command::new("claude")
            .args(["mcp", "add", "memex", path.to_str().unwrap_or(""), "mcp"])
            .output()?;
        std::io::Write::write_all(&mut std::io::stdout(), &out.stdout)?;
        std::io::Write::write_all(&mut std::io::stderr(), &out.stderr)?;
    }
    Ok(())
}

fn cmd_scan(
    path: Option<PathBuf>,
    agent: String,
    index: bool,
    limit: Option<usize>,
    redact: bool,
) -> Result<()> {
    // THR-05 — set the process-wide redaction gate before any indexing builds
    // vector extracts. Default ON; only `--redact=false` disables it.
    indexer::set_redaction_enabled(redact);
    if index && !redact {
        eprintln!("[memex scan] ⚠ secret redaction DISABLED (--redact=false) — secrets may be indexed");
    }
    let mut sessions: Vec<parser::Session> = if let Some(p) = path {
        eprintln!("scanning {} (single root)", p.display());
        scan_root_routed(&p)?
    } else {
        scan_by_agent(&agent)?
    };
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));

    let total = sessions.len();
    let to_show = limit.unwrap_or(total).min(total);

    println!(
        "{:<19} {:<24} {:<5} {:<5} {:<9} {:<11} title",
        "start", "project", "user", "asst", "tools", "branch"
    );
    println!("{}", "-".repeat(120));
    for s in sessions.iter().take(to_show) {
        println!("{}", parser::summary_line(s));
    }

    let tool_total: usize = sessions
        .iter()
        .flat_map(|s| s.turns.iter())
        .map(|t| t.tool_calls.len())
        .sum();
    eprintln!(
        "\nparsed {total} session(s) (shown: {to_show}), {tool_total} total tool calls"
    );

    if index {
        eprintln!("\nindexing into qdrant…");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .context("building tokio runtime")?;
        rt.block_on(async {
            let client = indexer::connect().await?;
            // P3 KG-03 dual-write: write path is v3, so v3 must exist.
            // The legacy v2 (`memex_sessions`) ensure stays for read-fallback
            // (KC-01b dual-read in retrieval.rs).
            crud::ensure_collection_v3(&client)
                .await
                .context("ensuring v3 collection before bulk index")?;
            indexer::ensure_collection(&client).await?;
            let embedder = std::sync::Arc::new(indexer::Embedder::new()?);
            // P5 — Arc-based bulk index uses embed_pool batching.
            let report = indexer::bulk_index_arc(&client, embedder, &sessions).await?;
            eprintln!(
                "\nindexed {}/{} session(s) into '{}' ({} duplicate sessionId(s) skipped, {} error(s))",
                report.indexed,
                total,
                crate::schema::COLLECTION_V3,
                report.duplicates_skipped,
                report.errors,
            );
            anyhow::Ok(())
        })?;
    }
    Ok(())
}

/// Dispatch `scan` by `--agent` flag. `claude` → Claude only;
/// `codex` → Codex only; anything else (default `all`) → both.
fn scan_by_agent(agent: &str) -> Result<Vec<parser::Session>> {
    let agent = agent.to_lowercase();
    match agent.as_str() {
        "claude" | "claude_code" => {
            let root = default_projects_root();
            eprintln!("scanning {} (claude only)", root.display());
            Ok(parser::scan_dir(&root)?)
        }
        "codex" => {
            let root = default_codex_root();
            eprintln!("scanning {} (codex only)", root.display());
            Ok(codex_parser::scan_codex_dir(&root)?)
        }
        _ => {
            let claude = default_projects_root();
            let codex = default_codex_root();
            eprintln!(
                "scanning {} + {} (all agents)",
                claude.display(),
                codex.display()
            );
            let mut all: Vec<parser::Session> = Vec::new();
            if claude.exists() {
                match parser::scan_dir(&claude) {
                    Ok(mut s) => all.append(&mut s),
                    Err(e) => eprintln!("  claude: {e:#}"),
                }
            }
            if codex.exists() {
                match codex_parser::scan_codex_dir(&codex) {
                    Ok(mut s) => all.append(&mut s),
                    Err(e) => eprintln!("  codex: {e:#}"),
                }
            }
            if all.is_empty() {
                anyhow::bail!("no sessions parsed under either root");
            }
            Ok(all)
        }
    }
}

/// Route a single explicit root by substring match on the path.
fn scan_root_routed(root: &std::path::Path) -> Result<Vec<parser::Session>> {
    let s = root.to_string_lossy();
    if s.contains("/.codex/sessions") {
        Ok(codex_parser::scan_codex_dir(root)?)
    } else {
        Ok(parser::scan_dir(root)?)
    }
}

fn default_codex_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".codex");
        p.push("sessions");
        p
    } else {
        PathBuf::from(".codex/sessions")
    }
}

fn cmd_search(query: String, limit: u64) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        let embedder = indexer::Embedder::new()?;
        let hits = indexer::search_content(&client, &embedder, &query, limit).await?;
        if hits.is_empty() {
            eprintln!("no results for {query:?}");
            return anyhow::Ok(());
        }
        println!(
            "{:<6} {:<19} {:<22} {:<40} session",
            "score", "start", "project", "title"
        );
        println!("{}", "-".repeat(120));
        for h in &hits {
            println!(
                "{:<6.4} {:<19} {:<22} {:<40} {}",
                h.score,
                h.start_iso
                    .get(..16)
                    .unwrap_or(&h.start_iso)
                    .replace('T', " "),
                truncate(&h.project_name, 22),
                truncate(if h.ai_title.is_empty() { "(untitled)" } else { &h.ai_title }, 40),
                h.session_id
            );
        }
        anyhow::Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
fn cmd_lens(
    query: String,
    content: f32,
    tool: f32,
    path: f32,
    error: f32,
    code: f32,
    limit: u64,
) -> Result<()> {
    let weights = indexer::LensWeights {
        content,
        tool,
        path,
        error,
        code,
        content_late: 0.0,
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        let embedder = indexer::Embedder::new()?;
        let hits = indexer::lens_search(&client, &embedder, &query, &weights, limit, 60).await?;
        print_hits_with_vectors(&hits);
        anyhow::Ok(())
    })
}

fn cmd_mix(positive: Vec<String>, negative: Vec<String>, limit: u64) -> Result<()> {
    if positive.is_empty() && negative.is_empty() {
        anyhow::bail!("provide at least --pos or --neg session ids");
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let client = indexer::connect().await?;
        let hits = indexer::mix_match(&client, &positive, &negative, limit).await?;
        print_hits(&hits);
        anyhow::Ok(())
    })
}

fn cmd_topology(sample: u32, per_point: u32, out: Option<PathBuf>) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let client = indexer::connect().await?;
        let topo = indexer::topology(&client, sample, per_point, Some(default_projects_root())).await?;
        eprintln!(
            "topology: {} node(s), {} MST edge(s), {} insight(s), {} gap(s)",
            topo.nodes.len(),
            topo.edges.len(),
            topo.project_insights.len(),
            topo.gap_insights.len(),
        );
        let json = serde_json::to_string_pretty(&topo)?;
        if let Some(p) = out {
            std::fs::write(&p, json)?;
            eprintln!("wrote {}", p.display());
        } else {
            println!("{json}");
        }
        anyhow::Ok(())
    })
}

fn cmd_predict(session_id: String, last_n: usize, horizon: usize, neighbors: u64) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        let embedder = indexer::Embedder::new()?;
        let ctx = indexer::predict_next_actions(
            &client, &embedder, &session_id, last_n, horizon, neighbors,
        )
        .await?;
        eprintln!(
            "predicting next actions for {} — looked at {} similar session(s), used {}",
            session_id, ctx.neighbors_searched, ctx.neighbors_used
        );
        if !ctx.source_last_turn_preview.is_empty() {
            eprintln!(
                "\nanchor (last turn): {}…",
                ctx.source_last_turn_preview.chars().take(120).collect::<String>()
            );
        }
        if ctx.predictions.is_empty() {
            eprintln!("\nno predictions — try a different anchor or re-index more sessions");
            return anyhow::Ok(());
        }
        println!(
            "\n{:<4} {:<14} {:<7} {:<7} {:<30} from-session"
            ,
            "#", "tool", "freq", "conf", "example"
        );
        println!("{}", "-".repeat(110));
        for p in &ctx.predictions {
            println!(
                "{:<4} {:<14} {:<7.2} {:<7.3} {:<30} {} (turn #{})",
                p.rank,
                truncate(&p.tool_name, 14),
                p.frequency,
                p.confidence,
                truncate(&p.example_input_summary, 30),
                truncate(&p.from_session_project, 22),
                p.from_turn_index
            );
        }
        anyhow::Ok(())
    })
}

/// Relevance-gate threshold for hook-mode recall. The UserPromptSubmit hook
/// fires on EVERY prompt, so we stay silent unless the top hit is genuinely
/// close — over-injection is the #1 reason users disable this hook. Tuned to
/// the same band the watcher's proactive recall uses (0.65).
const RECALL_HOOK_THRESHOLD: f32 = 0.65;

fn cmd_recall(
    error_text: Option<String>,
    limit: u64,
    cwd: Option<PathBuf>,
    hook: Option<String>,
    prompt: Option<String>,
) -> Result<()> {
    // Resolve the hook event (if any) up front.
    let hook_event = match hook.as_deref() {
        Some(h) => Some(
            crate::hook::HookEvent::parse(h)
                .ok_or_else(|| anyhow::anyhow!("unknown --hook event: {h}"))?,
        ),
        None => None,
    };
    // `--cwd` is accepted for hook ergonomics / future project-path scoping;
    // resolving it validates the arg even when unused for filtering today.
    let _resolved_cwd = match cwd.as_deref() {
        Some(_) => Some(companion::resolve_cwd_arg(cwd.as_deref())?),
        None => None,
    };

    // Determine the query text. In hook mode the query is the user's prompt:
    // prefer the explicit `--prompt`, else read the UserPromptSubmit JSON on
    // stdin and pull `.prompt`. Outside hook mode, `error_text` is required.
    let query: String = if hook_event.is_some() {
        if let Some(p) = prompt {
            p
        } else if let Some(p) = error_text.clone() {
            // Allow positional text in hook mode too (test ergonomics).
            p
        } else {
            read_prompt_from_stdin().unwrap_or_default()
        }
    } else {
        error_text
            .clone()
            .ok_or_else(|| anyhow::anyhow!("recall requires an error_text argument (or --hook + a prompt)"))?
    };

    // Empty / trivially-short query in hook mode → emit nothing (fail-open).
    if hook_event.is_some() && query.trim().chars().count() < 8 {
        return Ok(());
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        let embedder = indexer::Embedder::new()?;
        let hits = indexer::recall(&client, &embedder, &query, limit).await?;

        if let Some(event) = hook_event {
            // Relevance gate: only inject when the top hit clears the
            // threshold. Below it (or no hits) → silence is correct.
            let top_score = hits.first().map(|h| h.score).unwrap_or(0.0);
            if hits.is_empty() || top_score < RECALL_HOOK_THRESHOLD {
                return anyhow::Ok(());
            }
            let body = render_recall_hook_markdown(&query, &hits);
            crate::hook::emit(event, &body);
            return anyhow::Ok(());
        }

        if hits.is_empty() {
            eprintln!("no past sessions matched this error signature");
            return anyhow::Ok(());
        }
        eprintln!("recall — past sessions that may help:");
        print_hits(&hits);
        anyhow::Ok(())
    })
}

/// Read the UserPromptSubmit hook JSON from stdin and extract `.prompt`.
/// Returns None on any parse failure (fail-open — the caller treats an empty
/// prompt as "emit nothing").
fn read_prompt_from_stdin() -> Option<String> {
    use std::io::Read;
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(buf.trim()).ok()?;
    v.get("prompt")
        .and_then(|p| p.as_str())
        .map(str::to_string)
}

/// Compact markdown body for a relevance-gated recall injection. Kept tight
/// (UserPromptSubmit budget) — the top few past sessions that may help.
fn render_recall_hook_markdown(query: &str, hits: &[indexer::SearchHit]) -> String {
    let mut out = String::new();
    out.push_str("# 🔎 Memex recall — related past sessions\n");
    out.push_str(&format!(
        "Your prompt resembles {} past session(s); these may already hold the answer:\n",
        hits.len().min(3)
    ));
    for h in hits.iter().take(3) {
        let title = if h.ai_title.is_empty() { "(untitled)" } else { &h.ai_title };
        let head: String = h.session_id.chars().take(8).collect();
        out.push_str(&format!(
            "- `#{}` · {} · {} (sim {:.2})\n",
            head,
            truncate(&h.project_name, 28),
            truncate(title, 60),
            h.score,
        ));
    }
    let _ = query; // query is the trigger, not echoed back to avoid loops
    out
}

fn print_hits(hits: &[indexer::SearchHit]) {
    if hits.is_empty() {
        eprintln!("no results");
        return;
    }
    println!(
        "{:<7} {:<19} {:<22} {:<40} session",
        "score", "start", "project", "title"
    );
    println!("{}", "-".repeat(120));
    for h in hits {
        println!(
            "{:<7.4} {:<19} {:<22} {:<40} {}",
            h.score,
            h.start_iso
                .get(..16)
                .unwrap_or(&h.start_iso)
                .replace('T', " "),
            truncate(&h.project_name, 22),
            truncate(
                if h.ai_title.is_empty() {
                    "(untitled)"
                } else {
                    &h.ai_title
                },
                40,
            ),
            h.session_id
        );
    }
}

fn print_hits_with_vectors(hits: &[indexer::SearchHit]) {
    print_hits(hits);
    if let Some(top) = hits.first() {
        if !top.vector_scores.is_empty() {
            let mut keys: Vec<_> = top.vector_scores.keys().collect();
            keys.sort();
            let breakdown: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}={:.3}", k, top.vector_scores[k]))
                .collect();
            eprintln!("\ntop-hit per-vector contribution: {}", breakdown.join("  "));
        }
    }
}

fn cmd_snapshot(op: SnapshotOp) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        match op {
            SnapshotOp::Export { path } => {
                let name = indexer::snapshot_export(&path).await?;
                eprintln!("snapshot '{name}' exported to {}", path.display());
            }
            SnapshotOp::Import { path } => {
                indexer::snapshot_import(&path).await?;
                eprintln!("snapshot imported from {}", path.display());
            }
        }
        anyhow::Ok(())
    })
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn default_projects_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".claude");
        p.push("projects");
        p
    } else {
        PathBuf::from(".claude/projects")
    }
}
