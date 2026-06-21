use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::translation::sidecar::TranslationOutput;

/// Kills whatever process is listening on our port so a fresh server can be started.
/// Uses platform-specific commands (taskkill on Windows, lsof+kill on Unix).
pub fn kill_process_on_port() {
    let port = port();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x08000000;

        let mut netstat = Command::new("netstat");
        netstat.args(["-ano"]).creation_flags(NO_WINDOW);
        if let Ok(out) = netstat.output() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                if line.contains(&format!(":{port} ")) && line.contains("LISTENING") {
                    if let Some(pid_str) = line.split_whitespace().last() {
                        if let Ok(pid) = pid_str.parse::<u32>() {
                            let mut kill = Command::new("taskkill");
                            kill.args(["/F", "/PID", &pid.to_string()])
                                .creation_flags(NO_WINDOW);
                            let _ = kill.output();
                            std::thread::sleep(Duration::from_millis(300));
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("sh")
            .args([
                "-c",
                &format!("lsof -ti:{port} | xargs kill -9 2>/dev/null"),
            ])
            .output();
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn port() -> String {
    std::env::var("LT_ENGINE_PORT").unwrap_or_else(|_| "8765".to_string())
}

pub fn base_url() -> String {
    format!("http://127.0.0.1:{}", port())
}

/// Parses the /translate JSON response into a TranslationOutput.
pub fn parse_translate_response(body: &str) -> Result<TranslationOutput, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let wav = v
        .get("output_wav")
        .and_then(|x| x.as_str())
        .ok_or("response missing output_wav")?;
    let src = v
        .get("source_text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let tgt = v
        .get("translated_text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Ok(TranslationOutput {
        output_wav: PathBuf::from(wav),
        source_text: src,
        translated_text: tgt,
    })
}

/// Returns true if a server is already responding on /health.
pub fn is_server_up() -> bool {
    let client = reqwest::blocking::Client::new();
    client
        .get(format!("{}/health", base_url()))
        .timeout(std::time::Duration::from_millis(800))
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Spawns the FastAPI server as a child process.
/// Kills any leftover process on the port first so we always start fresh.
/// Stderr is piped and relayed to eprintln! so Python tracebacks appear in the dev console.
pub fn spawn_server() -> Result<Child, String> {
    kill_process_on_port();
    let program = crate::translation::sidecar::engine_python();
    let cwd = crate::translation::sidecar::python_dir();
    let mut cmd = Command::new(&program);
    cmd.args(["-m", "lt_engine.server"])
        .current_dir(cwd)
        .env(
            "PIPER_VOICE",
            crate::setup::piper_voice_path().to_string_lossy().as_ref(),
        )
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn translation server '{program}': {e}"))?;

    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                eprintln!("[lt_engine] {line}");
            }
        });
    }

    Ok(child)
}

/// Polls /health until ready or timeout.
pub fn wait_until_ready(timeout: Duration) -> Result<(), String> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/health", base_url());
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(resp) = client.get(&url).timeout(Duration::from_secs(2)).send() {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("translation server did not become ready in time".to_string())
}

/// Sends an audio file to the server for translation.
pub fn translate(input: &Path) -> Result<TranslationOutput, String> {
    if !input.exists() {
        return Err(format!("input file not found: {}", input.display()));
    }
    let ext_ok = input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "wav" | "mp3" | "flac" | "ogg" | "m4a"
            )
        })
        .unwrap_or(false);
    if !ext_ok {
        return Err(format!("unsupported audio file type: {}", input.display()));
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let out_dir = std::env::temp_dir().join(format!(
        "livetranslate-tr-{}-{}",
        std::process::id(),
        unique
    ));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{}/translate", base_url()))
        .json(&serde_json::json!({
            "input_path": input.to_string_lossy(),
            "out_dir": out_dir.to_string_lossy(),
            "src": "es",
            "tgt": "en"
        }))
        .timeout(Duration::from_secs(120))
        .send()
        .map_err(|e| format!("translation request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("translation server error {status}: {body}"));
    }
    let body = resp.text().map_err(|e| e.to_string())?;
    parse_translate_response(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_response() {
        let out = parse_translate_response(
            r#"{"output_wav":"C:/t/output.wav","source_text":"hola","translated_text":"hello"}"#,
        )
        .unwrap();
        assert_eq!(out.output_wav, PathBuf::from("C:/t/output.wav"));
        assert_eq!(out.translated_text, "hello");
    }

    #[test]
    fn missing_output_wav_errors() {
        assert!(parse_translate_response(r#"{"source_text":"x"}"#).is_err());
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_translate_response("nope").is_err());
    }
}
