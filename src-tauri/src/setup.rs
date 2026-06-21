use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

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

fn venv_python() -> PathBuf {
    let rel = if cfg!(windows) {
        ".venv-engine/Scripts/python.exe"
    } else {
        ".venv-engine/bin/python"
    };
    crate::translation::sidecar::repo_root().join(rel)
}

pub fn piper_voice_path() -> PathBuf {
    if let Ok(p) = std::env::var("PIPER_VOICE") {
        return PathBuf::from(p);
    }
    crate::translation::sidecar::repo_root().join("en_US-lessac-medium.onnx")
}

pub fn check() -> SetupStatus {
    let venv_ok = venv_python().exists();
    let piper_voice_ok =
        piper_voice_path().exists() && piper_voice_path().with_extension("onnx.json").exists();
    SetupStatus {
        venv_ok,
        piper_voice_ok,
        ready: venv_ok && piper_voice_ok,
    }
}

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

/// Downloads a file from `url` to `dest`, emitting progress events.
fn download_file(app: &AppHandle, url: &str, dest: &PathBuf, step: &str) -> Result<(), String> {
    use std::io::Write;

    emit_progress(app, step, 0, &format!("Downloading {url}"));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
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
    let mut buf = [0u8; 8192];

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

/// Downloads the Piper voice model and its config to the repo root.
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

/// Runs the full setup: venv creation + pip install + piper voice download.
/// Emits `setup-progress` and `setup-done` events on `app`.
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
    let root = crate::translation::sidecar::repo_root();
    let venv_python = venv_python();

    // Step 1 — create venv
    if !venv_python.exists() {
        emit_progress(app, "Creating Python environment", 5, "");

        let out = std::process::Command::new("python")
            .args(["-m", "venv", ".venv-engine"])
            .current_dir(&root)
            .output()
            .map_err(|e| format!("python not found: {e}"))?;

        if !out.status.success() {
            return Err(format!(
                "venv creation failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }

    // Step 2 — pip install torch (CPU)
    emit_progress(
        app,
        "Installing PyTorch (CPU)",
        15,
        "This may take a few minutes…",
    );
    run_pip(
        app,
        &venv_python,
        &[
            "install",
            "torch",
            "--index-url",
            "https://download.pytorch.org/whl/cpu",
        ],
        15,
        50,
    )?;

    // Step 3 — pip install remaining packages
    emit_progress(
        app,
        "Installing engine packages",
        55,
        "nemo, transformers, piper, fastapi…",
    );
    run_pip(
        app,
        &venv_python,
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

    // Step 4 — download piper voice
    emit_progress(app, "Downloading voice model", 88, "");
    download_piper_voice(app)?;

    emit_progress(app, "Setup complete", 100, "");
    Ok(())
}

/// Runs a pip subcommand, streaming output as progress detail events.
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

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    // Relay stderr lines as progress detail.
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

    // Drain stdout silently.
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
