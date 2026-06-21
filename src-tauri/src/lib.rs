mod audio;
mod setup;
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
async fn translate_file(
    input_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<TranslationFileResult, String> {
    // Clean up the temp dir from the previous offline translation before starting a new one.
    if let Ok(mut prev) = state.last_translation_out.lock() {
        if let Some(dir) = prev.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    let result = tauri::async_runtime::spawn_blocking(move || {
        translation::sidecar::translate_file(std::path::Path::new(&input_path))
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|o| TranslationFileResult {
        output_wav: o.output_wav.to_string_lossy().into_owned(),
        source_text: o.source_text,
        translated_text: o.translated_text,
    })?;

    // Track the new temp dir so it gets cleaned up on the next call.
    if let Ok(mut prev) = state.last_translation_out.lock() {
        let wav = std::path::PathBuf::from(&result.output_wav);
        *prev = wav.parent().map(|p| p.to_path_buf());
    }

    Ok(result)
}

#[tauri::command]
fn start_live_translation(
    device_name: String,
    output_device_name: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    // Wait for the engine server to be ready before starting capture.
    // warmup() can take 10–30s (Parakeet + Opus-MT + Piper on CPU).
    if !translation::engine_server::is_server_up() {
        let waited = std::time::Instant::now();
        translation::engine_server::wait_until_ready(std::time::Duration::from_secs(120))
            .map_err(|e| format!("translation server not ready after ~{}s: {e}", waited.elapsed().as_secs()))?;
    }
    let session = translation::live::start(&device_name, &output_device_name, app)?;
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

#[tauri::command]
fn check_setup() -> setup::SetupStatus {
    setup::check()
}

#[tauri::command]
fn start_setup(app: tauri::AppHandle) {
    setup::run_setup(app);
}

#[tauri::command]
fn check_vbcable() -> bool {
    audio::devices::list_output_devices()
        .iter()
        .any(|d| d.name.to_lowercase().contains("cable"))
}

#[tauri::command]
fn download_piper_voice(app: tauri::AppHandle) -> Result<(), String> {
    setup::download_piper_voice(&app)
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
            // Kill any leftover server from a previous run, then spawn fresh.
            match translation::engine_server::spawn_server() {
                Ok(child) => { *state.server.lock().unwrap() = Some(child); }
                Err(e) => { eprintln!("could not spawn translation server: {e}"); }
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
            stop_live_translation,
            check_setup,
            start_setup,
            check_vbcable,
            download_piper_voice,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
