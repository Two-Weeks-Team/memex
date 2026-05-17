//! `memex` CLI mode. Activates when the binary is invoked with a recognized
//! subcommand. Otherwise main.rs falls through to the Tauri GUI.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::parser;

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
        /// Cap the number of sessions printed (debug aid).
        #[arg(long)]
        limit: Option<usize>,
    },
}

pub fn run(args: Vec<String>) -> Result<()> {
    let cli = Cli::try_parse_from(args)?;
    match cli.command {
        Command::Scan { path, limit } => cmd_scan(path, limit),
    }
}

fn cmd_scan(path: Option<PathBuf>, limit: Option<usize>) -> Result<()> {
    let root = path.unwrap_or_else(default_projects_root);
    eprintln!("scanning {}", root.display());
    let mut sessions = parser::scan_dir(&root)?;
    // Stable order: most recent first.
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));

    let total = sessions.len();
    let to_show = limit.unwrap_or(total).min(total);

    println!(
        "{:<19} {:<24} {:<5} {:<5} {:<9} {:<11} {}",
        "start", "project", "user", "asst", "tools", "branch", "title"
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
        "\nparsed {} session(s) (shown: {}), {} total tool calls",
        total, to_show, tool_total
    );
    Ok(())
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
