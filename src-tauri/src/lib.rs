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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_output_devices,
            start_passthrough,
            stop_passthrough,
            translate_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
