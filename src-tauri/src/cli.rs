//! `memex` CLI mode. Activates when the binary is invoked with a recognized
//! subcommand. Otherwise main.rs falls through to the Tauri GUI.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::{indexer, parser};

#[derive(Debug, Parser)]
#[command(name = "memex", version, about = "Time Machine for AI session JSONL")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Walk a `~/.claude/projects` root and print a one-line summary per session.
    Scan {
        /// Path to scan. Defaults to `~/.claude/projects`.
        #[arg(long)]
        path: Option<PathBuf>,
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
    /// Snapshot management.
    Snapshot {
        #[command(subcommand)]
        op: SnapshotOp,
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
        Command::Scan { path, index, limit } => cmd_scan(path, index, limit),
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
        Command::Snapshot { op } => cmd_snapshot(op),
    }
}

fn cmd_scan(path: Option<PathBuf>, index: bool, limit: Option<usize>) -> Result<()> {
    let root = path.unwrap_or_else(default_projects_root);
    eprintln!("scanning {}", root.display());
    let mut sessions = parser::scan_dir(&root)?;
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
            indexer::ensure_collection(&client).await?;
            let embedder = indexer::Embedder::new()?;
            let report = indexer::bulk_index(&client, &embedder, &sessions).await?;
            eprintln!(
                "\nindexed {}/{} session(s) into '{}' ({} duplicate sessionId(s) skipped, {} error(s))",
                report.indexed,
                total,
                indexer::COLLECTION,
                report.duplicates_skipped,
                report.errors,
            );
            anyhow::Ok(())
        })?;
    }
    Ok(())
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
