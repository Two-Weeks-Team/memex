// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::process::ExitCode;

const CLI_SUBCOMMANDS: &[&str] = &[
    "scan", "search", "lens", "mix", "topology", "recall", "predict", "snapshot",
    "help", "--help", "-h",
];

fn is_cli_invocation() -> bool {
    env::args()
        .nth(1)
        .map(|a| CLI_SUBCOMMANDS.contains(&a.as_str()))
        .unwrap_or(false)
}

fn main() -> ExitCode {
    // macOS launches `.app` bundles with CWD=`/`, which is read-only on the
    // boot volume and breaks any default relative-path file operation
    // (fastembed cache, snapshot prompts, log files, etc.). Switch CWD to
    // $HOME so those defaults land somewhere writable. CLI invocations
    // already run from a writable CWD so leave them alone.
    if !is_cli_invocation() {
        if let Ok(home) = env::var("HOME") {
            let _ = env::set_current_dir(&home);
        }
    }

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
