mod audio;
mod setup;
mod state;
mod translation;

use state::{AppState, AudioCommand};
use tauri::{Emitter, Manager};

/// Recursively copies `src` into `dst`, creating dirs as needed.
/// Used in production to copy bundled Python source from resources to app data.
#[cfg(not(debug_assertions))]
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

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

/// Ensures the translation engine server is running, spawning it if needed.
/// Returns an immediate error if setup has not been completed yet.
fn ensure_server_running(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    if translation::engine_server::is_server_up() {
        return Ok(());
    }

    // If setup is not complete, tell the user right away instead of waiting 2 minutes.
    let status = setup::check();
    if !status.ready {
        return Err(
            "Setup not complete. Go to the Setup tab and run setup first.".to_string(),
        );
    }

    // Setup is done but the server isn't up — try to spawn (or re-spawn if it crashed).
    {
        let mut guard = state.server.lock().map_err(|e| e.to_string())?;
        let needs_spawn = match guard.as_mut() {
            None => true,
            Some(child) => matches!(child.try_wait(), Ok(Some(_))),
        };
        if needs_spawn {
            match translation::engine_server::spawn_server() {
                Ok(child) => {
                    *guard = Some(child);
                }
                Err(e) => return Err(format!("Could not start translation engine: {e}")),
            }
        }
    }

    // Let the frontend know warmup is in progress (models can take 30–90 s to load).
    let _ = app.emit("engine-starting", ());

    // Poll /health with process-liveness monitoring so a crashed Python process
    // fails fast (instead of waiting the full timeout). NeMo loads a 1.1 GB model
    // into RAM on first start, which can take several minutes on slow disks/CPUs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/health", translation::engine_server::base_url());
    while std::time::Instant::now() < deadline {
        // If the child process has exited, fail immediately.
        if let Ok(mut guard) = state.server.lock() {
            if let Some(ref mut child) = *guard {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!(
                        "Translation engine exited unexpectedly (code: {}). \
                         Check setup, logs above, and that no antivirus is blocking it.",
                        status.code().map(|c| c.to_string()).unwrap_or_else(|| "no code".into())
                    ));
                }
            }
        }

        if let Ok(resp) = client.get(&url).timeout(std::time::Duration::from_secs(2)).send() {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    Err(
        "Translation engine is taking too long to start. Check that setup completed successfully."
            .to_string(),
    )
}

/// Stops the live session and kills the engine server (and its whole process tree),
/// then frees the port. Called on app exit so no engine process is left behind.
fn shutdown_engine(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    // Stop any running live capture/translation session.
    if let Ok(mut g) = state.live.lock() {
        if let Some(sess) = g.take() {
            sess.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    // Kill the engine server. On Windows `child.kill()` only terminates the direct
    // process, so use taskkill /T to take down the whole tree (any pip/python helpers).
    if let Ok(mut g) = state.server.lock() {
        if let Some(mut child) = g.take() {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let mut k = std::process::Command::new("taskkill");
                k.args(["/F", "/T", "/PID", &child.id().to_string()])
                    .creation_flags(0x08000000);
                let _ = k.output();
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    // Belt-and-suspenders: free the port in case an orphan is still bound to it.
    translation::engine_server::kill_process_on_port();
}

#[tauri::command]
async fn start_live_translation(
    device_name: String,
    output_device_name: String,
    use_cloned_voice: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Run on a blocking thread so the engine readiness wait never freezes the UI.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        ensure_server_running(&app, &state)?;
        let session = translation::live::start(
            &device_name,
            &output_device_name,
            app.clone(),
            use_cloned_voice,
        )?;
        *state.live.lock().map_err(|e| e.to_string())? = Some(session);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
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
fn check_setup(app: tauri::AppHandle) -> setup::SetupStatus {
    setup::check_with_app(&app)
}

#[tauri::command]
fn start_setup(app: tauri::AppHandle) {
    setup::run_setup(app);
}

/// Spawns the engine server in the background (non-blocking) so models load while
/// the user finishes onboarding. By the time they click "Start", warmup is done.
/// Safe to call repeatedly: it no-ops if the server is already up or starting.
#[tauri::command]
fn warm_engine(state: tauri::State<AppState>) {
    if translation::engine_server::is_server_up() {
        return;
    }
    if !setup::check().ready {
        return;
    }
    let mut guard = match state.server.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let needs_spawn = match guard.as_mut() {
        None => true,
        Some(child) => matches!(child.try_wait(), Ok(Some(_))),
    };
    if needs_spawn {
        match translation::engine_server::spawn_server() {
            Ok(child) => {
                *guard = Some(child);
            }
            Err(e) => eprintln!("warm_engine: could not spawn server: {e}"),
        }
    }
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

#[tauri::command]
fn get_voice_profile_status() -> Result<bool, String> {
    translation::engine_server::voice_profile_exists()
}

#[tauri::command]
fn upload_voice_profile(audio_data: Vec<u8>) -> Result<(), String> {
    translation::engine_server::upload_voice_profile(&audio_data)
}

#[tauri::command]
fn delete_voice_profile() -> Result<(), String> {
    translation::engine_server::delete_voice_profile()
}

#[tauri::command]
async fn start_live_translation_ptt(
    device_name: String,
    output_device_name: String,
    use_cloned_voice: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        ensure_server_running(&app, &state)?;
        // Reset flag so we never start mid-recording state.
        state
            .ptt_recording
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let ptt_rec = state.ptt_recording.clone();
        let session = translation::live::start_ptt(
            &device_name,
            &output_device_name,
            app.clone(),
            ptt_rec,
            use_cloned_voice,
        )?;
        *state.live.lock().map_err(|e| e.to_string())? = Some(session);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
fn register_ptt_shortcut(
    shortcut_str: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Unregister previous PTT shortcut if any.
    if let Ok(guard) = state.ptt_shortcut.lock() {
        if let Some(ref prev) = *guard {
            app.global_shortcut().unregister(prev.as_str()).ok();
        }
    }

    app.global_shortcut()
        .register(shortcut_str.as_str())
        .map_err(|e| e.to_string())?;

    *state.ptt_shortcut.lock().map_err(|e| e.to_string())? = Some(shortcut_str);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // Push-to-talk (hold): record while the combo is held, send on release.
                    // Using store (not toggle) is robust to key-repeat — repeated Pressed
                    // events keep it true, and the Released event ends the take so the
                    // producer's falling edge flushes the recorded audio for translation.
                    let recording =
                        event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed;
                    let s = app.state::<AppState>();
                    s.ptt_recording
                        .store(recording, std::sync::atomic::Ordering::SeqCst);
                    let _ = app.emit("ptt-state", recording);
                })
                .build(),
        )
        .manage(AppState::default())
        .setup(|app| {
            use tauri::Manager;
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            // Production: resolve writable data dir and copy bundled Python source.
            #[cfg(not(debug_assertions))]
            {
                if let Ok(data_dir) = app.path().app_local_data_dir() {
                    let _ = std::fs::create_dir_all(&data_dir);
                    std::env::set_var("LT_ENGINE_ROOT", data_dir.to_string_lossy().as_ref());

                    // Copy lt_engine source from installer resources → data dir so the
                    // Python server can find it. Runs on every launch to pick up updates.
                    if let Ok(resource_dir) = app.path().resource_dir() {
                        let src = resource_dir.join("python");
                        let dst = data_dir.join("python");
                        if src.exists() {
                            if let Err(e) = copy_dir_all(&src, &dst) {
                                eprintln!("[LiveTranslate] failed to copy Python source from resources: {e}");
                            }
                        }
                    }
                }
            }

            let state = app.state::<AppState>();
            // Only pre-warm the server if setup is already complete.
            // On first run (no Python runtime yet) this would fail and is unnecessary.
            if setup::check().ready {
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
            }

            // ── System tray ────────────────────────────────────────────────
            let show_i = MenuItem::with_id(app, "show", "Open LiveTranslate", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu   = Menu::with_items(app, &[&show_i, &quit_i])?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("LiveTranslate")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => {
                        shutdown_engine(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // ── Close button shuts everything down (engine included) ───────
            let main_win   = app.get_webview_window("main").unwrap();
            let app_handle = app.handle().clone();
            main_win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    shutdown_engine(&app_handle);
                    app_handle.exit(0);
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
            start_live_translation_ptt,
            register_ptt_shortcut,
            check_setup,
            start_setup,
            warm_engine,
            check_vbcable,
            download_piper_voice,
            get_voice_profile_status,
            upload_voice_profile,
            delete_voice_profile,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
