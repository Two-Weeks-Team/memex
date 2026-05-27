// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::process::ExitCode;

const CLI_SUBCOMMANDS: &[&str] = &[
    "scan", "search", "lens", "mix", "topology", "recall", "predict", "snapshot",
    "serve", "warm-embedder", "mcp", "install-mcp", "memory", "wrapped",
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

    // fastembed → onnxruntime defaults to using every available CPU core,
    // which on a fresh-corpus warm-up (1 900+ legacy transcripts) pegs
    // every core at 100 % and turns the fan into a jet engine. Cap the
    // intra-op thread pool so the user's machine stays usable. The user
    // can override via `OMP_NUM_THREADS=...` if they want the old behavior.
    for var in ["OMP_NUM_THREADS", "ORT_INTRA_OP_NUM_THREADS", "ORT_NUM_THREADS"] {
        if env::var_os(var).is_none() {
            // SAFETY: called before any threads are spawned (main thread,
            // before Tauri runtime / fastembed init). Modifying the env
            // here is the only reliable way to influence the C++ runtime.
            unsafe { env::set_var(var, "2") };
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
        #[cfg(feature = "gui")]
        {
            memex_lib::run();
            ExitCode::SUCCESS
        }
        #[cfg(not(feature = "gui"))]
        {
            eprintln!(
                "memex: headless (web) build — no GUI. Use a subcommand, e.g. `memex serve` or `memex mcp` (see `memex --help`)."
            );
            ExitCode::FAILURE
        }
    }
}
