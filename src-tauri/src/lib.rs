mod audio;
mod state;
mod translation;

use state::{AppState, AudioCommand};

#[tauri::command]
fn get_output_devices() -> Vec<String> {
    audio::devices::list_output_devices()
        .into_iter()
        .map(|d| d.name)
        .collect()
}

#[tauri::command]
fn start_passthrough(output_name: String, state: tauri::State<AppState>) -> Result<(), String> {
    let (resp_tx, resp_rx) = std::sync::mpsc::channel();
    state.send_command(AudioCommand::Start {
        output_name,
        respond: resp_tx,
    })?;
    resp_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn stop_passthrough(state: tauri::State<AppState>) -> Result<(), String> {
    let (resp_tx, resp_rx) = std::sync::mpsc::channel();
    state.send_command(AudioCommand::Stop { respond: resp_tx })?;
    resp_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct TranslationFileResult {
    output_wav: String,
    source_text: String,
    translated_text: String,
}

#[tauri::command]
async fn translate_file(input_path: String) -> Result<TranslationFileResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        translation::sidecar::translate_file(std::path::Path::new(&input_path))
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|o| TranslationFileResult {
        output_wav: o.output_wav.to_string_lossy().into_owned(),
        source_text: o.source_text,
        translated_text: o.translated_text,
    })
}

#[tauri::command]
fn start_live_translation(
    device_name: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let session = translation::live::start(&device_name, app)?;
    *state.live.lock().map_err(|e| e.to_string())? = Some(session);
    Ok(())
}

#[tauri::command]
fn stop_live_translation(state: tauri::State<AppState>) {
    if let Ok(mut g) = state.live.lock() {
        if let Some(sess) = g.take() {
            sess.stop
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            use tauri::Manager;
            let state = app.state::<AppState>();
            match translation::engine_server::spawn_server() {
                Ok(child) => {
                    *state.server.lock().unwrap() = Some(child);
                }
                Err(e) => {
                    eprintln!("could not spawn translation server: {e}");
                }
            }
            std::thread::spawn(|| {
                if let Err(e) = translation::engine_server::wait_until_ready(
                    std::time::Duration::from_secs(120),
                ) {
                    eprintln!("translation server not ready: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_output_devices,
            start_passthrough,
            stop_passthrough,
            translate_file,
            start_live_translation,
            stop_live_translation
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
