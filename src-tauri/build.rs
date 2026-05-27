fn main() {
    // tauri_build is only needed for the GUI (Tauri) build. The `web` build
    // (`--no-default-features --features web`) links no Tauri, so skip it —
    // Cargo sets CARGO_FEATURE_GUI when the `gui` feature is active.
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        tauri_build::build();
    }
}
