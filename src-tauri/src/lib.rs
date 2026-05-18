pub mod cli;
pub mod codex_parser;
pub mod commands;
pub mod crud;
pub mod embed_late;
pub mod embed_pool;
pub mod enrich;
pub mod eval_ndcg;
pub mod indexer;
pub mod insights_cache;
pub mod lens;
pub mod parse_cache;
pub mod parser;
pub mod retrieval;
pub mod schema;
pub mod sec;
pub mod snapshot;

use std::sync::Arc;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

use crate::commands::{AppState, AppStateArc};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // AppState is managed eagerly with EMPTY lazy slots. Qdrant and
            // fastembed init lazily on the first command that needs them, so
            // the window can open instantly and the app self-heals if the
            // user starts Qdrant after launching Memex.
            app.manage::<AppStateArc>(Arc::new(AppState::new()));
            eprintln!("[memex] AppState registered (qdrant + embedder will init on first use)");

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
