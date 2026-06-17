use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use crate::translation::sidecar::TranslationOutput;

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

/// Spawns the FastAPI server as a child process.
pub fn spawn_server() -> Result<Child, String> {
    let program = crate::translation::sidecar::engine_python();
    let cwd = crate::translation::sidecar::python_dir();
    Command::new(&program)
        .args(["-m", "lt_engine.server"])
        .current_dir(cwd)
        .spawn()
        .map_err(|e| format!("failed to spawn translation server '{program}': {e}"))
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
