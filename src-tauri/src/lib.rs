mod audio;
mod state;

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
    state
        .sender
        .lock()
        .map_err(|e| e.to_string())?
        .send(AudioCommand::Start {
            output_name,
            respond: resp_tx,
        })
        .map_err(|e| e.to_string())?;
    resp_rx.recv().map_err(|e| e.to_string())?
}

#[tauri::command]
fn stop_passthrough(state: tauri::State<AppState>) -> Result<(), String> {
    state
        .sender
        .lock()
        .map_err(|e| e.to_string())?
        .send(AudioCommand::Stop)
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_output_devices,
            start_passthrough,
            stop_passthrough
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
