pub mod cli;
pub mod codex_parser;
#[cfg(feature = "gui")]
pub mod commands;
pub mod companion;
pub mod crud;
pub mod embed_late;
pub mod embed_pool;
pub mod enrich;
pub mod eval_ndcg;
pub mod indexer;
pub mod insights_cache;
pub mod lens;
pub mod mcp;
pub mod parse_cache;
pub mod parser;
pub mod payload;
pub mod retrieval;
pub mod schema;
pub mod sec;
pub mod snapshot;
pub mod summary;
#[cfg(feature = "gui")]
pub mod watcher;
pub mod wrapped;
#[cfg(feature = "web")]
pub mod web;

#[cfg(feature = "gui")]
use std::{path::PathBuf, sync::Arc, time::Duration};

#[cfg(feature = "gui")]
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

#[cfg(feature = "gui")]
use crate::commands::{AppState, AppStateArc};

#[cfg(feature = "gui")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // P8 — `memex://` URL scheme with 5 surface routes:
        //   memex://timemachine   memex://topology   memex://lens
        //   memex://predict       memex://mix-match
        // The plugin emits a `deep-link://new-url` event the frontend listens
        // to in `src/main.js` (`__TAURI__.event.listen('deep-link://new-url')`)
        // and dispatches to the matching tab. macOS uses `Info.plist`
        // `CFBundleURLTypes` (configured via tauri.conf.json `plugins`).
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // AppState is managed eagerly with EMPTY lazy slots. Qdrant and
            // fastembed init lazily on the first command that needs them, so
            // the window can open instantly and the app self-heals if the
            // user starts Qdrant after launching Memex.
            let app_state: AppStateArc = Arc::new(AppState::new());
            app.manage::<AppStateArc>(app_state.clone());
            eprintln!("[memex] AppState registered (qdrant + embedder will init on first use)");

            // Background auto-index daemon. Walks ~/.claude/projects every
            // 60 s, re-indexes any session whose mtime advanced. Emits
            // `index-updated` so the frontend can flash a chip.
            let watch_root = default_projects_root();
            // Floor at 1 s so a misconfigured env var (e.g. =0) can't turn
            // the watcher into a CPU-pegging busy loop.
            const MIN_WATCH_PERIOD_SECS: u64 = 1;
            let raw_period = std::env::var("MEMEX_WATCHER_PERIOD_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60);
            let watch_period = Duration::from_secs(raw_period.max(MIN_WATCH_PERIOD_SECS));
            watcher::start_watcher(
                app_state.clone(),
                app.handle().clone(),
                watch_root,
                watch_period,
            );

            // Tray icon — minimal Open / Snapshot / Quit menu.
            let open_item = MenuItem::with_id(app, "open", "Open Memex", true, None::<&str>)?;
            let snap_item = MenuItem::with_id(app, "snapshot", "Export Snapshot…", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &snap_item, &quit_item])?;
            let _tray = TrayIconBuilder::with_id("memex-tray")
                .tooltip("Memex")
                .icon(app.default_window_icon().cloned().expect("default icon"))
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "snapshot" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.eval(
                                "document.getElementById('btn-snapshot')?.click();",
                            );
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::lens_search,
            commands::mix_match,
            commands::topology,
            commands::recall,
            commands::get_session,
            commands::get_session_turns,
            commands::snapshot_export,
            commands::snapshot_export_default,
            commands::snapshot_import,
            commands::collection_info,
            commands::refresh_index,
            commands::tail_recent_errors,
            commands::list_sessions,
            commands::predict_next_actions,
            // P4 advanced retrieval
            commands::mix_match_with_pairs,
            commands::list_sessions_ordered,
            commands::lens_search_grouped,
            commands::relevance_feedback,
            // P2 KA-01/02/05 — FormulaQuery-backed lens with score breakdown
            commands::lens_search_v2,
            commands::prompt_history_stats,
            // Memex Companion — Cold Start Killer.
            commands::compose_memory_primer,
            // Memex Wrapped — engineering "Spotify Wrapped".
            commands::compose_wrapped,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(feature = "gui")]
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
