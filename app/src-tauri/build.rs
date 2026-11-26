//! Declares the app's command allowlist. Each command below gets its own
//! `allow-…` permission, and only the ones granted in
//! `capabilities/default.json` can be called from the web view.

const COMMANDS: &[&str] = &[
    "get_state",
    "get_settings",
    "save_settings",
    "get_history",
    "clear_history",
    "app_info",
    "complete_word",
    "check_code",
    "create_code",
    "join_code",
    "confirm_safety",
    "send_paths",
    "send_text",
    "respond_offer",
    "pause_transfer",
    "resume_transfer",
    "cancel_transfer",
    "end_session",
    "dismiss_session",
    "open_folder",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    tauri_build::try_build(attributes).expect("failed to run the Tauri build script");
}
