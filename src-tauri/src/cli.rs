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
    Recall {
        error_text: String,
        #[arg(long, default_value_t = 5)]
        limit: u64,
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
        Command::Scan { path, agent, index, limit } => cmd_scan(path, agent, index, limit),
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
        Command::Recall { error_text, limit } => cmd_recall(error_text, limit),
        Command::Predict {
            session_id,
            last_n,
            horizon,
            neighbors,
        } => cmd_predict(session_id, last_n, horizon, neighbors),
        Command::Snapshot { op } => cmd_snapshot(op),
        Command::Mcp => cmd_mcp(),
        Command::InstallMcp { run } => cmd_install_mcp(run),
        Command::Memory { cwd, limit, json } => cmd_memory(cwd, limit, json),
        Command::Wrapped { window_days, limit, json } => cmd_wrapped(window_days, limit, json),
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

fn cmd_memory(cwd: Option<PathBuf>, limit: usize, json: bool) -> Result<()> {
    let resolved = companion::resolve_cwd_arg(cwd.as_deref())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        let embedder = indexer::Embedder::new()?;
        let primer =
            companion::compose_memory_primer(&client, &embedder, &resolved, limit).await?;
        eprintln!(
            "[memex memory] cwd={} sessions_used={}/{} decisions={} pitfalls={} ({} ms)",
            primer.cwd,
            primer.stats.neighbors_used,
            primer.stats.neighbors_searched,
            primer.stats.decisions_extracted,
            primer.stats.pitfalls_extracted,
            primer.elapsed_ms,
        );
        if json {
            println!("{}", serde_json::to_string_pretty(&primer)?);
        } else {
            print!("{}", primer.markdown);
        }
        anyhow::Ok(())
    })
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
) -> Result<()> {
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

fn cmd_recall(error_text: String, limit: u64) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let client = indexer::connect().await?;
        let embedder = indexer::Embedder::new()?;
        let hits = indexer::recall(&client, &embedder, &error_text, limit).await?;
        if hits.is_empty() {
            eprintln!("no past sessions matched this error signature");
            return anyhow::Ok(());
        }
        eprintln!("recall — past sessions that may help:");
        print_hits(&hits);
        anyhow::Ok(())
    })
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
