pub mod cli;
pub mod commands;
pub mod indexer;
pub mod parser;

use std::sync::Arc;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

use crate::commands::{AppState, AppStateArc};

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Initialize Qdrant client + embedder once, share via State.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match init_app_state().await {
                    Ok(state) => {
                        handle.manage::<AppStateArc>(Arc::new(state));
                        eprintln!("[memex] AppState ready (qdrant + embedder)");
                    }
                    Err(e) => {
                        eprintln!("[memex] AppState init FAILED: {e:#}");
                    }
                }
            });

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
            greet,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

async fn init_app_state() -> anyhow::Result<AppState> {
    let qdrant = indexer::connect().await?;
    indexer::ensure_collection(&qdrant).await?;
    let embedder = indexer::Embedder::new()?;
    Ok(AppState { qdrant, embedder })
}
