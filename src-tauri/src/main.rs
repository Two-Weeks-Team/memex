// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::process::ExitCode;

const CLI_SUBCOMMANDS: &[&str] = &["scan", "search", "snapshot", "help", "--help", "-h"];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let want_cli = args
        .get(1)
        .map(|a| CLI_SUBCOMMANDS.contains(&a.as_str()))
        .unwrap_or(false);

    if want_cli {
        match memex_lib::cli::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("memex: {e:#}");
                ExitCode::FAILURE
            }
        }
    } else {
        memex_lib::run();
        ExitCode::SUCCESS
    }
}
