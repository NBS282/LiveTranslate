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

/// The renamed Python interpreter shown in Task Manager as "livetranslate-engine".
fn engine_exe() -> PathBuf {
    engine_dir().join("livetranslate-engine.exe")
}

/// Active Python interpreter: portable runtime (production) or venv (dev).
fn venv_python() -> PathBuf {
    let prod = engine_exe();
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

fn download_file(app: &AppHandle, url: &str, dest: &PathBuf, step: &str) -> Result<(), String> {
    use std::io::Write;

    emit_progress(app, step, 0, &format!("Downloading {url}"));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("download returned HTTP {}", resp.status()));
    }

    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 65536];

    loop {
        use std::io::Read;
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        if total > 0 {
            let pct = ((downloaded * 100) / total) as u8;
            emit_progress(
                app,
                step,
                pct,
                &format!("{:.1} / {:.1} MB", mb(downloaded), mb(total)),
            );
        }
    }

    Ok(())
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

// ── Portable Python (production only) ────────────────────────────────────────

/// Downloads python-build-standalone, extracts it, and creates livetranslate-engine.exe.
/// Uses Windows' built-in tar.exe (available since Windows 10 v1803).
fn download_portable_python(app: &AppHandle) -> Result<(), String> {
    const URL: &str = concat!(
        "https://github.com/astral-sh/python-build-standalone/releases/download/",
        "20250409/cpython-3.11.12+20250409-x86_64-pc-windows-msvc-install_only.tar.gz"
    );

    let root = crate::translation::sidecar::repo_root();
    let engine_root = root.join("engine");
    std::fs::create_dir_all(&engine_root).map_err(|e| e.to_string())?;

    let archive = engine_root.join("python-portable.tar.gz");

    // Download (~30 MB).
    download_file(app, URL, &archive, "Downloading Python runtime")?;

    // Extract with Windows built-in tar (Win 10 v1803+).
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
        .map_err(|e| format!("tar not found (requires Windows 10 v1803+): {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "tar extraction failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    // Clean up archive to save ~30 MB.
    let _ = std::fs::remove_file(&archive);

    // Copy python.exe → livetranslate-engine.exe so Task Manager shows the app name.
    let python_exe = engine_dir().join("python.exe");
    let lt_exe = engine_exe();
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

    // Bootstrap pip (not included in install_only variant).
    emit_progress(app, "Bootstrapping pip", 8, "");
    let mut pip_bootstrap = std::process::Command::new(&lt_exe);
    pip_bootstrap.args(["-m", "ensurepip", "--upgrade"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        pip_bootstrap.creation_flags(0x08000000);
    }
    let bootstrap = pip_bootstrap.output().map_err(|e| e.to_string())?;
    if !bootstrap.status.success() {
        return Err(format!(
            "ensurepip failed: {}",
            String::from_utf8_lossy(&bootstrap.stderr)
        ));
    }

    Ok(())
}

/// Rewrites the engine binary's PE version-info and icon via bundled rcedit, so the
/// OS task list shows "LiveTranslate" with our logo instead of "Python".
/// Best-effort: failures are ignored (the app still works, just keeps the Python name).
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

    if !onnx.exists() {
        download_file(
            app,
            &format!("{base}/en_US-lessac-medium.onnx"),
            &onnx,
            "Downloading Piper voice model",
        )?;
    }
    if !json.exists() {
        download_file(
            app,
            &format!("{base}/en_US-lessac-medium.onnx.json"),
            &json,
            "Downloading Piper voice config",
        )?;
    }
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
    if is_production() && !engine_exe().exists() {
        emit_progress(app, "Downloading Python runtime", 3, "~30 MB…");
        download_portable_python(app)?;
    }

    // ── Step 1 (dev only): create venv ───────────────────────────────────────
    if !python.exists() && !is_production() {
        emit_progress(app, "Creating Python environment", 5, "");
        let root = crate::translation::sidecar::repo_root();
        let mut venv_cmd = std::process::Command::new("python");
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
        &[
            "install",
            "torch",
            "--index-url",
            "https://download.pytorch.org/whl/cpu",
        ],
        15,
        50,
    )?;

    // ── Step 3: install remaining engine packages ─────────────────────────────
    emit_progress(
        app,
        "Installing engine packages",
        55,
        "nemo, transformers, piper, fastapi…",
    );
    run_pip(
        app,
        &python,
        &[
            "install",
            "nemo_toolkit[asr]",
            "transformers",
            "sentencepiece",
            "piper-tts",
            "fastapi[standard]",
            "uvicorn[standard]",
            "soundfile",
        ],
        55,
        85,
    )?;

    // ── Step 4: download Piper voice model ────────────────────────────────────
    emit_progress(app, "Downloading voice model", 88, "");
    download_piper_voice(app)?;

    // ── Step 5: ensure lt_engine source is available ──────────────────────────
    // In production, lib.rs copies the bundled python/ from resources to data_dir on
    // every launch, but that copy can fail silently. We verify here so the server can
    // always find the lt_engine package (via PYTHONPATH) after setup finishes.
    let python_src = crate::translation::sidecar::python_dir();
    if !python_src.exists() {
        emit_progress(app, "Copying engine source", 97, "");
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
