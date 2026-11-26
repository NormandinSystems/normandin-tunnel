//! The desktop shell around the Tunnel engine.
//!
//! The web view never touches the network or the file system. It can call the
//! short list of commands below (see `build.rs` and `capabilities/default.json`
//! for the allowlist) and it listens to one event, `tunnel-event`. Every
//! argument is validated here, in Rust, before the engine sees it.

#![deny(unsafe_code)]

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tunnel_core::{
    CodeCheck, Config, DeviceType, Error, Event, HistoryEntry, Settings, Snapshot, TransferState,
    Tunnel,
};

/// Name of the event that carries engine events to the web view.
const EVENT: &str = "tunnel-event";
/// Longest list of paths accepted from one drop or file dialog.
const MAX_PATHS: usize = 10_000;

struct AppState {
    tunnel: Tunnel,
}

type CommandResult<T> = Result<T, Error>;

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    platform: &'static str,
    data_dir: String,
    default_save_dir: String,
    system_language: String,
}

#[tauri::command]
fn get_state(state: State<'_, AppState>) -> Snapshot {
    // The first call means the web view loaded the interface and can reach
    // the engine. The start-up check in CI waits for this line.
    static FIRST_CALL: std::sync::Once = std::sync::Once::new();
    FIRST_CALL.call_once(|| tracing::info!("interface ready"));
    state.tunnel.snapshot()
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.tunnel.settings()
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings) -> CommandResult<Settings> {
    state.tunnel.save_settings(settings)
}

#[tauri::command]
fn get_history(state: State<'_, AppState>) -> CommandResult<Vec<HistoryEntry>> {
    state.tunnel.history(200)
}

#[tauri::command]
fn clear_history(state: State<'_, AppState>) -> CommandResult<()> {
    state.tunnel.clear_history()
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        data_dir: tunnel_core::platform::display_path(&tunnel_core::platform::default_data_dir()),
        default_save_dir: tunnel_core::platform::display_path(
            &tunnel_core::platform::default_save_dir(),
        ),
        system_language: system_language(),
    }
}

#[tauri::command]
fn complete_word(prefix: String) -> Vec<&'static str> {
    if prefix.len() > 32 {
        return Vec::new();
    }
    Tunnel::complete_word(&prefix, 6)
}

#[tauri::command]
fn check_code(state: State<'_, AppState>, input: String) -> CommandResult<CodeCheck> {
    if input.len() > 512 {
        return Err(Error::CodeInvalid);
    }
    state.tunnel.check_code(&input)
}

#[tauri::command]
async fn create_code(state: State<'_, AppState>) -> CommandResult<()> {
    state.tunnel.create_code().await
}

#[tauri::command]
async fn join_code(
    state: State<'_, AppState>,
    input: String,
    allow_other_host: bool,
) -> CommandResult<()> {
    if input.len() > 512 {
        return Err(Error::CodeInvalid);
    }
    state.tunnel.join_code(&input, allow_other_host).await
}

#[tauri::command]
async fn confirm_safety(state: State<'_, AppState>, matches: bool) -> CommandResult<()> {
    state.tunnel.confirm_safety(matches).await
}

#[tauri::command]
async fn send_paths(state: State<'_, AppState>, paths: Vec<String>) -> CommandResult<String> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(Error::NothingToSend);
    }
    let mut checked = Vec::with_capacity(paths.len());
    for path in paths {
        let path = PathBuf::from(path);
        // Only real, absolute paths: what a file dialog or a drop produces.
        if !path.is_absolute() || std::fs::symlink_metadata(&path).is_err() {
            return Err(Error::NothingToSend);
        }
        checked.push(path);
    }
    state.tunnel.send_paths(checked).await
}

#[tauri::command]
async fn send_text(state: State<'_, AppState>, text: String) -> CommandResult<()> {
    state.tunnel.send_text(text).await
}

#[tauri::command]
async fn respond_offer(
    state: State<'_, AppState>,
    id: String,
    accept: bool,
    folder: Option<String>,
) -> CommandResult<()> {
    state
        .tunnel
        .respond_offer(&id, accept, folder.map(PathBuf::from))
        .await
}

#[tauri::command]
async fn pause_transfer(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.tunnel.pause(&id).await
}

#[tauri::command]
async fn resume_transfer(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.tunnel.resume(&id).await
}

#[tauri::command]
async fn cancel_transfer(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.tunnel.cancel(&id).await
}

#[tauri::command]
async fn end_session(state: State<'_, AppState>) -> CommandResult<()> {
    state.tunnel.end_session().await;
    Ok(())
}

#[tauri::command]
fn dismiss_session(state: State<'_, AppState>) {
    state.tunnel.dismiss();
}

/// Opens a folder in the system file manager. Folders only: a received file is
/// never opened or run by the app.
#[tauri::command]
fn open_folder(path: String) -> CommandResult<()> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_dir() {
        return Err(Error::DestinationNotWritable);
    }
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|err| Error::Io(err.to_string()))
}

fn system_language() -> String {
    let locale = sys_locale::get_locale().unwrap_or_default().to_lowercase();
    if locale.starts_with("fr") {
        "fr".into()
    } else {
        "en".into()
    }
}

fn language(settings: &Settings) -> String {
    match settings.language.as_str() {
        "en" | "fr" => settings.language.clone(),
        _ => system_language(),
    }
}

fn size_text(bytes: u64, french: bool) -> String {
    let units: [&str; 5] = if french {
        ["o", "Ko", "Mo", "Go", "To"]
    } else {
        ["B", "KB", "MB", "GB", "TB"]
    };
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < units.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", units[0])
    } else if french {
        format!("{value:.1} {}", units[unit]).replace('.', ",")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// Shows a system notification for things that need attention while the
/// window is in the background.
fn notify(app: &AppHandle, tunnel: &Tunnel, event: &Event) {
    let focused = app
        .get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    let settings = tunnel.settings();
    if focused || !settings.notifications {
        return;
    }
    let french = language(&settings) == "fr";
    let (title, body) = match event {
        Event::Offer { offer } => {
            let size = size_text(offer.bytes, french);
            if french {
                (
                    format!("{} veut vous envoyer des fichiers", offer.peer.name),
                    format!(
                        "{} fichier(s), {size}. Ouvrez Tunnel pour répondre.",
                        offer.files
                    ),
                )
            } else {
                (
                    format!("{} wants to send you files", offer.peer.name),
                    format!("{} file(s), {size}. Open Tunnel to answer.", offer.files),
                )
            }
        }
        Event::Transfer { transfer } if transfer.state == TransferState::Done => {
            let size = size_text(transfer.bytes_total, french);
            if french {
                (
                    "Transfert terminé".to_string(),
                    format!("{} fichier(s), {size}.", transfer.files_done),
                )
            } else {
                (
                    "Transfer finished".to_string(),
                    format!("{} file(s), {size}.", transfer.files_done),
                )
            }
        }
        Event::Transfer { transfer } if transfer.state == TransferState::Failed => {
            if french {
                (
                    "Transfert interrompu".to_string(),
                    "Ouvrez Tunnel pour voir les détails.".to_string(),
                )
            } else {
                (
                    "Transfer interrupted".to_string(),
                    "Open Tunnel to see the details.".to_string(),
                )
            }
        }
        Event::Text { message } if message.from_peer => {
            if french {
                (
                    "Nouveau message".to_string(),
                    "Ouvrez Tunnel pour le lire.".to_string(),
                )
            } else {
                (
                    "New message".to_string(),
                    "Open Tunnel to read it.".to_string(),
                )
            }
        }
        _ => return,
    };
    let _ = app.notification().builder().title(title).body(body).show();
}

fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Starts the app.
pub fn run() {
    let log_filter = std::env::var("TUNNEL_LOG").unwrap_or_else(|_| "warn".into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(log_filter))
        .with_writer(std::io::stderr)
        .try_init();

    let mut builder = tauri::Builder::default();
    // A second copy of the app focuses the first window instead. Set
    // TUNNEL_DATA_DIR to run a separate copy for testing on one machine.
    let separate_copy = std::env::var_os("TUNNEL_DATA_DIR").is_some();
    if !separate_copy {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            focus_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let mut config = Config {
                device_type: DeviceType::Desktop,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                include_loopback: std::env::var_os("TUNNEL_LOOPBACK").is_some(),
                rendezvous_url: std::env::var("TUNNEL_RENDEZVOUS").ok(),
                ..Config::default()
            };
            if let Some(dir) = std::env::var_os("TUNNEL_DATA_DIR") {
                config.data_dir = PathBuf::from(dir);
            }
            if std::env::var_os("TUNNEL_NO_RELAY").is_some() {
                config.relay = Some(tunnel_core::RelayChoice::Disabled);
            }
            let tunnel = Tunnel::open(config)?;

            // Forward engine events to the web view.
            let mut events = tunnel.subscribe();
            let handle = app.handle().clone();
            let forwarded = tunnel.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            notify(&handle, &forwarded, &event);
                            let _ = handle.emit(EVENT, &event);
                        }
                        // The web view fell behind: tell it to reload the whole picture.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = handle.emit(EVENT, serde_json::json!({ "type": "resync" }));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            app.manage(AppState { tunnel });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_settings,
            save_settings,
            get_history,
            clear_history,
            app_info,
            complete_word,
            check_code,
            create_code,
            join_code,
            confirm_safety,
            send_paths,
            send_text,
            respond_offer,
            pause_transfer,
            resume_transfer,
            cancel_transfer,
            end_session,
            dismiss_session,
            open_folder,
        ])
        .on_window_event(|window, event| {
            // Closing the window ends the session cleanly: the other device is
            // told, and no partial files are left behind.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if state.tunnel.has_session() {
                    api.prevent_close();
                    let tunnel = state.tunnel.clone();
                    let window = window.clone();
                    tauri::async_runtime::spawn(async move {
                        let ending = tunnel.end_session();
                        let _ =
                            tokio::time::timeout(std::time::Duration::from_secs(5), ending).await;
                        let _ = window.destroy();
                    });
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start Normandin Tunnel");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_localised() {
        assert_eq!(size_text(999, false), "999 B");
        assert_eq!(size_text(1_500_000, false), "1.5 MB");
        assert_eq!(size_text(1_500_000, true), "1,5 Mo");
        assert_eq!(size_text(12, true), "12 o");
    }
}
