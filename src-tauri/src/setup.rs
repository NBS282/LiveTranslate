use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Clone)]
pub struct SetupStatus {
    pub venv_ok: bool,
    pub piper_voice_ok: bool,
    pub ready: bool,
}

#[derive(Serialize, Clone)]
pub struct SetupProgress {
    pub step: String,
    pub percent: u8,
    pub detail: String,
}

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Root of the python-build-standalone extraction: <LT_ENGINE_ROOT>/engine/python/
fn engine_dir() -> PathBuf {
    crate::translation::sidecar::repo_root()
        .join("engine")
        .join("python")
}

/// The production portable Python interpreter: `livetranslate-engine.exe` on
/// Windows (renamed + rebranded, see `rebrand_engine_exe`), `bin/python3` on
/// macOS (python-build-standalone's Unix `install_only` layout, unmodified).
/// See `sidecar::production_python_rel_path` for the OS selection logic.
fn production_python_path() -> PathBuf {
    engine_dir().join(crate::translation::sidecar::production_python_rel_path(
        std::env::consts::OS,
    ))
}

/// Active Python interpreter: portable runtime (production) or venv (dev).
fn venv_python() -> PathBuf {
    let prod = production_python_path();
    if prod.exists() {
        return prod;
    }
    let rel = if cfg!(windows) {
        ".venv-engine/Scripts/python.exe"
    } else {
        ".venv-engine/bin/python"
    };
    crate::translation::sidecar::repo_root().join(rel)
}

/// Voice model lives in <LT_ENGINE_ROOT>/models/ so it survives app updates.
pub fn piper_voice_path() -> PathBuf {
    if let Ok(p) = std::env::var("PIPER_VOICE") {
        return PathBuf::from(p);
    }
    let models_dir = crate::translation::sidecar::repo_root().join("models");
    let _ = std::fs::create_dir_all(&models_dir);
    models_dir.join("en_US-lessac-medium.onnx")
}

/// True when running as a packaged/installed app (LT_ENGINE_ROOT is set by lib.rs).
fn is_production() -> bool {
    std::env::var("LT_ENGINE_ROOT").is_ok()
}

// ── Status check ─────────────────────────────────────────────────────────────

pub fn check() -> SetupStatus {
    let venv_ok = venv_python().exists();
    let voice = piper_voice_path();
    let piper_voice_ok = voice.exists() && voice.with_extension("onnx.json").exists();
    SetupStatus {
        venv_ok,
        piper_voice_ok,
        ready: venv_ok && piper_voice_ok,
    }
}

/// Setup status resolved from the canonical per-user data directory.
///
/// `repo_root()` relies on the `LT_ENGINE_ROOT` env var, which is set in lib.rs
/// during app startup. If that var is ever missing on a launch, `repo_root()`
/// falls back to the executable's directory — which has no `engine/` or `models/`,
/// so `check()` wrongly reports "not installed" and the user is sent back to setup
/// on every reopen. Resolving the data dir straight from the AppHandle here (and
/// re-asserting the env var) makes the readiness check immune to that timing issue.
pub fn check_with_app(app: &AppHandle) -> SetupStatus {
    #[cfg(not(debug_assertions))]
    if let Ok(dir) = app.path().app_local_data_dir() {
        std::env::set_var("LT_ENGINE_ROOT", dir.to_string_lossy().as_ref());
    }
    let _ = app; // used only in release builds
    check()
}

// ── Progress events ───────────────────────────────────────────────────────────

fn emit_progress(app: &AppHandle, step: &str, percent: u8, detail: &str) {
    let _ = app.emit(
        "setup-progress",
        SetupProgress {
            step: step.to_string(),
            percent,
            detail: detail.to_string(),
        },
    );
}

// ── File download ─────────────────────────────────────────────────────────────
//
// The resumable download core (retry/backoff, `.part` file, HEAD size check)
// lives in `crate::net::download` so it can be shared with
// `models::manager::ModelManager`. This module only supplies the
// Tauri-event progress reporting on top of it.

use crate::net::download::{self, DEFAULT_MAX_ATTEMPTS};

const DOWNLOAD_MAX_ATTEMPTS: u32 = DEFAULT_MAX_ATTEMPTS;

fn needs_download(local_size: Option<u64>, remote_size: Option<u64>) -> bool {
    download::needs_download(local_size, remote_size)
}

/// Size the server reports for `url`, via a HEAD request. Best-effort.
fn remote_content_length(url: &str) -> Option<u64> {
    download::remote_content_length(url)
}

fn download_file(
    app: &AppHandle,
    url: &str,
    dest: &std::path::Path,
    step: &str,
) -> Result<(), String> {
    emit_progress(app, step, 0, &format!("Downloading {url}"));

    let app_retry = app.clone();
    let step_retry = step.to_string();
    let app_progress = app.clone();
    let step_progress = step.to_string();

    download::download_with_progress(
        url,
        dest,
        DOWNLOAD_MAX_ATTEMPTS,
        move |attempt| {
            emit_progress(
                &app_retry,
                &step_retry,
                0,
                &format!("Retrying download (attempt {attempt}/{DOWNLOAD_MAX_ATTEMPTS})…"),
            );
        },
        move |downloaded, total| {
            if total > 0 {
                let pct = ((downloaded * 100) / total).min(100) as u8;
                emit_progress(
                    &app_progress,
                    &step_progress,
                    pct,
                    &format!("{:.1} / {:.1} MB", mb(downloaded), mb(total)),
                );
            }
        },
    )
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

// ── Portable Python (production only) ────────────────────────────────────────

/// Builds the `pip install torch ...` argument list for the given
/// `std::env::consts::OS`-style tag. Pure function so both branches are
/// tested regardless of host OS.
///
/// Windows (and Linux, were it ever supported) need the `+cpu` wheel index —
/// PyPI's default `torch` wheel there pulls a multi-GB CUDA dependency tree
/// we don't want. macOS has no CUDA wheel variant to begin with: PyPI's
/// default `torch` wheel for macOS is already the arm64 build (CPU +
/// Accelerate/MPS), so overriding the index would be a no-op at best and a
/// broken URL at worst (`download.pytorch.org/whl/cpu` doesn't publish
/// macOS wheels).
fn torch_install_args(os: &str) -> Vec<&'static str> {
    let mut args = vec!["install", "torch"];
    if os != "macos" {
        args.extend(["--index-url", "https://download.pytorch.org/whl/cpu"]);
    }
    args
}

/// Resolves the python-build-standalone `install_only` tarball URL for the
/// given `std::env::consts::OS`-style tag. Pure function (no I/O) so the URL
/// for a platform we're not currently running on is still covered by tests —
/// a typo in the macOS URL would otherwise only be discovered by the user's
/// Mac, not by Windows-side CI.
///
/// Only Windows (x86_64) and macOS (aarch64 / Apple Silicon) are supported —
/// this is a beta scoped to Apple Silicon Macs; Intel Macs are out of scope
/// (see docs/MACOS.md).
fn python_build_standalone_url(os: &str) -> Result<&'static str, String> {
    match os {
        "windows" => Ok(concat!(
            "https://github.com/astral-sh/python-build-standalone/releases/download/",
            "20250409/cpython-3.11.12+20250409-x86_64-pc-windows-msvc-install_only.tar.gz"
        )),
        "macos" => Ok(concat!(
            "https://github.com/astral-sh/python-build-standalone/releases/download/",
            "20250409/cpython-3.11.12+20250409-aarch64-apple-darwin-install_only.tar.gz"
        )),
        other => Err(format!(
            "no portable Python runtime is available for target OS '{other}' \
             (LiveTranslate's portable-runtime setup only supports Windows x86_64 \
             and macOS aarch64/Apple Silicon)"
        )),
    }
}

/// Downloads python-build-standalone and extracts it into `engine/python/`.
/// Uses the platform's built-in `tar` (Windows 10 v1803+ ships one; macOS's
/// BSD `tar` handles `.tar.gz` natively) — no bundled extraction tool needed.
fn download_portable_python(app: &AppHandle) -> Result<(), String> {
    let url = python_build_standalone_url(std::env::consts::OS)?;

    let root = crate::translation::sidecar::repo_root();
    let engine_root = root.join("engine");
    std::fs::create_dir_all(&engine_root).map_err(|e| e.to_string())?;

    let archive = engine_root.join("python-portable.tar.gz");

    // Download (~20-30 MB depending on platform).
    download_file(app, url, &archive, "Downloading Python runtime")?;

    // Extract with the OS's built-in tar.
    emit_progress(app, "Extracting Python runtime", 5, "");
    let mut tar_cmd = std::process::Command::new("tar");
    tar_cmd.args([
        "-xzf",
        &archive.to_string_lossy(),
        "-C",
        &engine_root.to_string_lossy(),
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        tar_cmd.creation_flags(0x08000000);
    }
    let out = tar_cmd
        .output()
        .map_err(|e| format!("tar not found (requires Windows 10 v1803+ or macOS): {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "tar extraction failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    // Clean up archive to save space.
    let _ = std::fs::remove_file(&archive);

    finalize_portable_python(app)?;

    Ok(())
}

/// Windows: renames `python.exe` → `livetranslate-engine.exe` (so Task
/// Manager shows the app name), rebrands its PE resources, then bootstraps
/// pip (the `install_only` Windows tarball ships without it).
#[cfg(windows)]
fn finalize_portable_python(app: &AppHandle) -> Result<PathBuf, String> {
    // Copy python.exe → livetranslate-engine.exe so Task Manager shows the app name.
    let python_exe = engine_dir().join("python.exe");
    let lt_exe = production_python_path();
    if python_exe.exists() && !lt_exe.exists() {
        std::fs::copy(&python_exe, &lt_exe)
            .map_err(|e| format!("failed to create livetranslate-engine.exe: {e}"))?;
    }

    // Rebrand the binary's PE resources so Task Manager shows "LiveTranslate" and
    // our icon instead of "Python". Renaming the file alone is not enough — the
    // task list reads FileDescription/ProductName and the embedded icon.
    if lt_exe.exists() {
        rebrand_engine_exe(app, &lt_exe);
    }

    bootstrap_pip(app, &lt_exe)?;
    Ok(lt_exe)
}

/// macOS: python-build-standalone's Unix `install_only` layout already
/// places a ready-to-use interpreter at `bin/python3` (a symlink to the
/// pinned `python3.NN` binary) with pip pre-installed — no renaming or PE
/// rebranding applies outside Windows, so this is just a passthrough plus a
/// pip bootstrap for defense in depth (ensurepip is a local, no-network
/// operation, so it's cheap even though pip is already present).
#[cfg(not(windows))]
fn finalize_portable_python(app: &AppHandle) -> Result<PathBuf, String> {
    let python3 = production_python_path();
    bootstrap_pip(app, &python3)?;
    Ok(python3)
}

/// Ensures `pip` is importable for `python`. Local/offline (no network call) —
/// `ensurepip --upgrade` only reinstalls the wheel bundled with the
/// interpreter, it does not fetch anything from PyPI.
fn bootstrap_pip(app: &AppHandle, python: &std::path::Path) -> Result<(), String> {
    emit_progress(app, "Bootstrapping pip", 8, "");
    let mut cmd = std::process::Command::new(python);
    cmd.args(["-m", "ensurepip", "--upgrade"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "ensurepip failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Rewrites the engine binary's PE version-info and icon via bundled rcedit, so the
/// OS task list shows "LiveTranslate" with our logo instead of "Python".
/// Best-effort: failures are ignored (the app still works, just keeps the Python name).
#[cfg(windows)]
fn rebrand_engine_exe(app: &AppHandle, exe: &PathBuf) {
    let Ok(res_dir) = app.path().resource_dir() else {
        return;
    };
    let rcedit = res_dir.join("rcedit.exe");
    if !rcedit.exists() {
        return;
    }

    let mut cmd = std::process::Command::new(&rcedit);
    cmd.arg(exe)
        .args(["--set-version-string", "FileDescription", "LiveTranslate"])
        .args(["--set-version-string", "ProductName", "LiveTranslate"])
        .args(["--set-version-string", "CompanyName", "LiveTranslate"])
        .args([
            "--set-version-string",
            "InternalName",
            "livetranslate-engine",
        ])
        .args([
            "--set-version-string",
            "OriginalFilename",
            "livetranslate-engine.exe",
        ]);

    let ico = res_dir.join("app-icon.ico");
    if ico.exists() {
        cmd.args(["--set-icon", &ico.to_string_lossy()]);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let _ = cmd.output();
}

// ── Piper voice download ──────────────────────────────────────────────────────

pub fn download_piper_voice(app: &AppHandle) -> Result<(), String> {
    let base = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium";
    let onnx = piper_voice_path();
    let json = onnx.with_extension("onnx.json");

    let targets = [
        (
            format!("{base}/en_US-lessac-medium.onnx"),
            onnx,
            "Downloading Piper voice model",
        ),
        (
            format!("{base}/en_US-lessac-medium.onnx.json"),
            json,
            "Downloading Piper voice config",
        ),
    ];

    for (url, path, step) in targets {
        let local = std::fs::metadata(&path).ok().map(|m| m.len());
        // Compare against the size the server reports so a previously truncated
        // download gets repaired instead of being trusted forever.
        let remote = remote_content_length(&url);
        if needs_download(local, remote) {
            download_file(app, &url, &path, step)?;
        }
    }
    Ok(())
}

// ── Native STT GGUF model download ───────────────────────────────────────────

/// Downloads (and verifies) the native STT backend's default GGUF model via
/// `ModelManager`, using the same catalog id resolution `engine::build` uses
/// (`resolve_native_model_id` — env override `LT_NATIVE_STT_MODEL`, else the
/// Q8 Parakeet default). A no-op if the file is already present and matches
/// the catalog's expected size.
///
/// This mirrors `engine::build_native`'s download step but never loads the
/// model into memory — setup only needs the bytes on disk and verified,
/// loading happens lazily when a live session actually starts.
pub fn download_native_stt_model(app: &AppHandle) -> Result<(), String> {
    let model_id = crate::translation::engine::resolve_native_model_id();
    let entry = crate::models::catalog::find(&model_id)
        .ok_or_else(|| format!("unknown native STT model id: {model_id}"))?;

    let manager = crate::models::manager::ModelManager::new(
        crate::models::manager::ModelManager::default_root(app),
    );

    if manager.is_downloaded(entry) {
        return Ok(());
    }

    // This step owns the [89, 90] slice of the overall setup bar (the caller
    // emits 89 before invoking us); scale byte progress into it instead of
    // emitting a raw 0-100 that would visibly jump the bar backwards.
    const PCT_START: u8 = 89;
    const PCT_END: u8 = 90;

    let step = "Downloading native STT model";
    emit_progress(
        app,
        step,
        PCT_START,
        &format!("{model_id} ({:.0} MB)…", mb(entry.size_bytes)),
    );

    let app_progress = app.clone();
    let step_progress = step.to_string();
    manager.download(entry, move |downloaded, total| {
        if let Some(ratio) = downloaded
            .checked_mul(100)
            .and_then(|d| d.checked_div(total))
        {
            let inner = ratio.min(100) as u8;
            emit_progress(
                &app_progress,
                &step_progress,
                scale_progress(PCT_START, PCT_END, inner),
                &format!("{:.1} / {:.1} MB", mb(downloaded), mb(total)),
            );
        }
    })?;

    manager.verify(entry)?;
    Ok(())
}

// ── Main setup entry point ────────────────────────────────────────────────────

pub fn run_setup(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(e) = run_setup_inner(&app) {
            let _ = app.emit(
                "setup-done",
                serde_json::json!({ "success": false, "error": e }),
            );
        } else {
            let _ = app.emit("setup-done", serde_json::json!({ "success": true }));
        }
    });
}

fn run_setup_inner(app: &AppHandle) -> Result<(), String> {
    let python = venv_python();

    // ── Step 0 (production): portable Python runtime ──────────────────────────
    if is_production() && !production_python_path().exists() {
        emit_progress(app, "Downloading Python runtime", 3, "~30 MB…");
        download_portable_python(app)?;
    }

    // ── Step 1 (dev only): create venv ───────────────────────────────────────
    if !python.exists() && !is_production() {
        emit_progress(app, "Creating Python environment", 5, "");
        let root = crate::translation::sidecar::repo_root();
        // Windows installs expose `python`; macOS/Linux only ship `python3`.
        let system_python = if cfg!(windows) { "python" } else { "python3" };
        let mut venv_cmd = std::process::Command::new(system_python);
        venv_cmd
            .args(["-m", "venv", ".venv-engine"])
            .current_dir(&root);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            venv_cmd.creation_flags(0x08000000);
        }
        let out = venv_cmd
            .output()
            .map_err(|e| format!("python not found: {e}"))?;

        if !out.status.success() {
            return Err(format!(
                "venv creation failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }

    let python = venv_python();

    // ── Step 1.5: upgrade pip to latest ──────────────────────────────────────
    // ensurepip --upgrade only ships the pip version bundled with Python, not
    // the latest from PyPI. hf_xet (and some nemo deps) need a modern pip with
    // PEP 517 build-backend support.
    emit_progress(app, "Upgrading pip", 13, "");
    run_pip(app, &python, &["install", "--upgrade", "pip"], 13, 15)?;

    // ── Step 1.6 (Windows): ensure VC++ 2015-2022 x64 runtime is present ─────
    // PyTorch DLLs (c10.dll, torch_cpu.dll, …) link against msvcp140.dll and
    // vcruntime140.dll. On a fresh Windows install those are missing. We detect
    // the absence and silently install the official redistributable before pip
    // installs PyTorch so the import never fails with WinError 126.
    #[cfg(windows)]
    if !std::path::Path::new("C:\\Windows\\System32\\msvcp140.dll").exists() {
        emit_progress(
            app,
            "Installing Visual C++ Runtime",
            12,
            "Required by PyTorch — ~25 MB, one-time install…",
        );
        let tmp = std::env::temp_dir().join("vc_redist_lt_x64.exe");
        download_file(
            app,
            "https://aka.ms/vs/17/release/vc_redist.x64.exe",
            &tmp,
            "Installing Visual C++ Runtime",
        )?;
        let mut cmd = std::process::Command::new(&tmp);
        // /install /quiet /norestart: silently elevate + install, no reboot.
        cmd.args(["/install", "/quiet", "/norestart"]);
        let out = cmd
            .output()
            .map_err(|e| format!("failed to run vc_redist: {e}"))?;
        let _ = std::fs::remove_file(&tmp);
        let code = out.status.code().unwrap_or(-1);
        // 0=ok  3010=ok(restart advised)  1638=already installed
        if code != 0 && code != 3010 && code != 1638 {
            return Err(format!(
                "Visual C++ Runtime installation failed (code {code}).\n\
                 Please install it manually and retry:\n\
                 https://aka.ms/vs/17/release/vc_redist.x64.exe"
            ));
        }
    }

    // ── Step 2: install PyTorch (CPU) ─────────────────────────────────────────
    emit_progress(
        app,
        "Installing PyTorch (CPU)",
        15,
        "This may take a few minutes…",
    );
    run_pip(
        app,
        &python,
        &torch_install_args(std::env::consts::OS),
        15,
        50,
    )?;

    // ── Step 3: install remaining engine packages ─────────────────────────────
    // `nemo_toolkit[asr]` (NeMo Parakeet ASR) is intentionally NOT installed
    // here anymore: native `transcribe.cpp` (GGUF Parakeet, see Step 4.5
    // below) is the default STT backend since Phase 5, and NeMo's PyTorch +
    // CUDA-adjacent dependency tree was the single largest install/download
    // cost this process paid for a path that isn't the default anymore. It
    // remains an opt-in for `LT_STT_BACKEND=python`: install it manually.
    emit_progress(
        app,
        "Installing engine packages",
        55,
        "transformers, piper, fastapi…",
    );
    run_pip(
        app,
        &python,
        &[
            "install",
            "transformers",
            "sentencepiece",
            "piper-tts",
            "fastapi[standard]",
            "uvicorn[standard]",
            "soundfile",
            // Pin huggingface_hub to a version where the Windows symlink
            // fallback (WinError 1314) uses correct absolute paths internally.
            "huggingface_hub>=0.25",
            // Enables Xet chunked parallel downloads from HuggingFace Hub
            // (helps the MarianMT + Pocket TTS downloads below).
            "hf_xet",
        ],
        55,
        85,
    )?;

    // ── Step 3.5: install Pocket TTS voice-cloning engine (optional) ─────────
    // Voice cloning is an opt-in feature, so this install is best-effort: if it
    // fails (e.g. a transitive dep that pip can't resolve), the core app still
    // works with Piper. We surface the failure as a progress note instead of
    // aborting setup.
    emit_progress(
        app,
        "Installing voice cloning engine",
        86,
        "Optional — enables cloning your own voice…",
    );
    if let Err(e) = run_pip(app, &python, &["install", "pocket-tts"], 86, 88) {
        emit_progress(
            app,
            "Voice cloning unavailable",
            88,
            &format!(
                "Optional engine skipped (pocket-tts): {e}. The app works with the standard voice."
            ),
        );
    }

    // ── Step 4: download Piper voice model ────────────────────────────────────
    emit_progress(app, "Downloading voice model", 88, "");
    download_piper_voice(app)?;

    // ── Step 4.5: download native STT GGUF model ──────────────────────────────
    // The native transcribe.cpp cascade (default STT backend since Phase 5)
    // resolves this same model id via ModelManager on first use if it isn't
    // already here — downloading it during setup avoids a multi-minute,
    // progress-bar-less stall on the first live-translation attempt for
    // fresh installs. Existing installs that already have `nemo_toolkit`
    // (pre-Phase-5) and no GGUF yet still get it lazily on first native
    // session, or by re-running setup.
    emit_progress(app, "Downloading native STT model", 89, "");
    download_native_stt_model(app)?;

    // ── Step 5: ensure lt_engine source is available ──────────────────────────
    // In production, lib.rs copies the bundled python/ from resources to data_dir on
    // every launch, but that copy can fail silently. We verify here — and BEFORE the
    // model download, which runs `-m lt_engine.setup_models` and needs the package.
    let python_src = crate::translation::sidecar::python_dir();
    if !python_src.exists() {
        emit_progress(app, "Copying engine source", 90, "");
        match app.path().resource_dir() {
            Ok(res_dir) => {
                let bundled = res_dir.join("python");
                if bundled.exists() {
                    copy_dir_recursive(&bundled, &python_src)?;
                } else {
                    return Err("Engine source (lt_engine) not found in app resources. \
                         Please reinstall LiveTranslate."
                        .to_string());
                }
            }
            Err(e) => {
                return Err(format!("Cannot locate app resources: {e}"));
            }
        }
    }

    // ── Step 6: pre-cache ML models (MarianMT + Pocket TTS) ────────────────────
    // Models are stored in <LT_ENGINE_ROOT>/models/hf/ so they stay with the
    // app data and are never re-downloaded on subsequent launches. The
    // orchestration lives in python/lt_engine/setup_models.py (testable with
    // pytest); it downloads two models at a time and reports PROGRESS:<pct>.
    // Native STT/AST GGUF models are NOT downloaded here: Parakeet is handled
    // by Step 4.5 above (`download_native_stt_model`), and Canary downloads
    // lazily on first `LT_TRANSLATION_ENGINE=canary` session.
    let hf_cache = crate::translation::sidecar::repo_root()
        .join("models")
        .join("hf");
    let _ = std::fs::create_dir_all(&hf_cache);
    let hf_cache_str = hf_cache.to_string_lossy().into_owned();

    emit_progress(
        app,
        "Downloading translation models",
        91,
        "~2.0 GB — first-time download, please wait…",
    );
    // NEMO_CACHE_DIR is no longer read by setup_models.py (Parakeet's NeMo
    // download was removed — see download_native_stt_model above), but is
    // still passed through: it's a harmless no-op env var, and existing
    // installs' `nemo_toolkit` (if present) still respects it for the
    // python STT fallback (LT_STT_BACKEND=python).
    let nemo_cache_str = hf_cache.join("nemo").to_string_lossy().into_owned();
    let _ = std::fs::create_dir_all(hf_cache.join("nemo"));
    let python_src_str = python_src.to_string_lossy().into_owned();

    run_python_module(app, &python, "lt_engine.setup_models", 91, 97, &{
        let mut env_pairs = vec![
            ("PYTHONPATH", python_src_str.as_str()),
            ("HF_HOME", hf_cache_str.as_str()),
            ("TRANSFORMERS_CACHE", hf_cache_str.as_str()),
            ("NEMO_CACHE_DIR", nemo_cache_str.as_str()),
            ("HF_HUB_DISABLE_SYMLINKS_WARNING", "1"),
        ];
        // Only pass HF_TOKEN when a non-empty one was baked in at build
        // time. The Hub rejects EVERY download (including public repos)
        // with 401 when the token is empty, expired, or revoked.
        if let Some(token) = option_env!("HF_TOKEN").filter(|t| !t.is_empty()) {
            env_pairs.push(("HF_TOKEN", token));
        }
        env_pairs
    })?;

    emit_progress(app, "Setup complete", 100, "");
    Ok(())
}

// ── Directory copy ────────────────────────────────────────────────────────────

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let dst_path = dst.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ── pip runner ────────────────────────────────────────────────────────────────

fn run_pip(
    app: &AppHandle,
    python: &PathBuf,
    args: &[&str],
    pct_start: u8,
    _pct_end: u8,
) -> Result<(), String> {
    use std::io::BufRead;
    use std::process::Stdio;

    let mut cmd = std::process::Command::new(python);
    cmd.arg("-m").arg("pip");
    for a in args {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    if let Some(stderr) = child.stderr.take() {
        let app2 = app.clone();
        let step = args.first().copied().unwrap_or("pip").to_string();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                emit_progress(&app2, &step, pct_start, &line);
            }
        });
    }

    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            std::io::BufReader::new(stdout).lines().for_each(|_| {});
        });
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("pip {} failed (exit {:?})", args[0], status.code()));
    }
    Ok(())
}

// ── Python module runner ──────────────────────────────────────────────────────

/// Maps a subprocess-reported percentage (0-100) into the [pct_start, pct_end]
/// slice this step occupies on the overall setup progress bar.
fn scale_progress(pct_start: u8, pct_end: u8, inner: u8) -> u8 {
    let span = pct_end.saturating_sub(pct_start) as u32;
    pct_start + ((inner.min(100) as u32 * span) / 100) as u8
}

fn run_python_module(
    app: &AppHandle,
    python: &PathBuf,
    module: &str,
    pct_start: u8,
    pct_end: u8,
    extra_env: &[(&str, &str)],
) -> Result<(), String> {
    use std::io::BufRead;
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};

    let mut cmd = std::process::Command::new(python);
    // -u: unbuffered so progress lines arrive in real-time on Windows.
    cmd.args(["-u", "-m", module])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Prevent codec crashes on Windows when NeMo/HF print non-ASCII chars.
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    // Capture stderr for the error message AND stream it as progress events.
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let stderr_thread = if let Some(stderr) = child.stderr.take() {
        let app2 = app.clone();
        let cap = captured.clone();
        Some(std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                emit_progress(&app2, "Downloading models", pct_start, &line);
                if let Ok(mut v) = cap.lock() {
                    v.push(line);
                }
            }
        }))
    } else {
        None
    };

    let stdout_thread = if let Some(stdout) = child.stdout.take() {
        let app2 = app.clone();
        Some(std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                // PROGRESS:<pct> lines are the module's overall progress
                // (cache-size based); everything else is log detail.
                if let Some(rest) = line.strip_prefix("PROGRESS:") {
                    if let Ok(inner) = rest.trim().parse::<u8>() {
                        let pct = scale_progress(pct_start, pct_end, inner);
                        emit_progress(&app2, "Downloading models", pct, "");
                        continue;
                    }
                }
                emit_progress(&app2, "Downloading models", pct_start, &line);
            }
        }))
    } else {
        None
    };

    let status = child.wait().map_err(|e| e.to_string())?;

    // Wait for I/O threads to drain before reading captured lines.
    if let Some(h) = stderr_thread {
        let _ = h.join();
    }
    if let Some(h) = stdout_thread {
        let _ = h.join();
    }

    if !status.success() {
        let lines = captured.lock().map(|v| v.clone()).unwrap_or_default();
        let n = lines.len();
        let tail = lines[n.saturating_sub(10)..].join("\n");
        return Err(if tail.is_empty() {
            format!("model pre-download failed (exit {:?})", status.code())
        } else {
            format!(
                "model pre-download failed (exit {:?}):\n{tail}",
                status.code()
            )
        });
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_build_standalone_url_windows_targets_msvc_triple() {
        let url = python_build_standalone_url("windows").unwrap();
        assert!(url.contains("x86_64-pc-windows-msvc-install_only.tar.gz"));
    }

    #[test]
    fn python_build_standalone_url_macos_targets_apple_silicon() {
        let url = python_build_standalone_url("macos").unwrap();
        assert!(url.contains("aarch64-apple-darwin-install_only.tar.gz"));
    }

    #[test]
    fn python_build_standalone_url_rejects_unsupported_os() {
        assert!(python_build_standalone_url("linux").is_err());
        assert!(python_build_standalone_url("ios").is_err());
    }

    #[test]
    fn python_build_standalone_urls_share_the_same_pinned_release() {
        // Windows and macOS must stay on the same CPython version — a
        // divergent pin here would only surface as behavior drift discovered
        // during the user's Mac UAT, not in CI.
        let windows = python_build_standalone_url("windows").unwrap();
        let macos = python_build_standalone_url("macos").unwrap();
        assert!(windows.contains("cpython-3.11.12+20250409-"));
        assert!(macos.contains("cpython-3.11.12+20250409-"));
    }

    #[test]
    fn torch_install_args_windows_uses_cpu_wheel_index() {
        let args = torch_install_args("windows");
        assert_eq!(
            args,
            vec![
                "install",
                "torch",
                "--index-url",
                "https://download.pytorch.org/whl/cpu"
            ]
        );
    }

    #[test]
    fn torch_install_args_macos_uses_default_pypi_index() {
        let args = torch_install_args("macos");
        assert_eq!(args, vec!["install", "torch"]);
    }

    #[test]
    fn needs_download_when_local_missing() {
        assert!(needs_download(None, Some(10)));
        assert!(needs_download(None, None));
    }

    #[test]
    fn trusts_local_file_when_remote_size_unknown() {
        assert!(!needs_download(Some(10), None));
    }

    #[test]
    fn redownloads_on_size_mismatch() {
        assert!(needs_download(Some(10), Some(20)));
    }

    #[test]
    fn skips_download_when_sizes_match() {
        assert!(!needs_download(Some(10), Some(10)));
    }

    // `backoff_delay` and `part_path` themselves now live in
    // `crate::net::download` (with their own tests there); these two keep
    // setup.rs's original test names/behavior green through the re-export.
    #[test]
    fn backoff_doubles_per_retry() {
        assert_eq!(download::backoff_delay(1).as_secs(), 2);
        assert_eq!(download::backoff_delay(2).as_secs(), 4);
        assert_eq!(download::backoff_delay(3).as_secs(), 8);
    }

    #[test]
    fn part_path_appends_suffix_without_touching_extension() {
        let p = download::part_path(std::path::Path::new("C:/models/voice.onnx"));
        assert!(p.to_string_lossy().ends_with("voice.onnx.part"));
        let q = download::part_path(std::path::Path::new("C:/models/voice.onnx.json"));
        assert!(q.to_string_lossy().ends_with("voice.onnx.json.part"));
    }

    #[test]
    fn scale_progress_maps_into_step_slice() {
        assert_eq!(scale_progress(91, 97, 0), 91);
        assert_eq!(scale_progress(91, 97, 50), 94);
        assert_eq!(scale_progress(91, 97, 100), 97);
        // Values above 100 from a buggy reporter are clamped, not overflowed.
        assert_eq!(scale_progress(91, 97, 255), 97);
        // Inverted ranges degrade to the start instead of panicking.
        assert_eq!(scale_progress(97, 91, 50), 97);
    }
}
