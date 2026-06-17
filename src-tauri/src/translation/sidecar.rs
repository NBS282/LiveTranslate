use std::path::{Path, PathBuf};

/// Result of a successful offline translation.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOutput {
    pub output_wav: PathBuf,
    pub source_text: String,
    pub translated_text: String,
}

/// Parses the engine's result.json content into (source_text, translated_text).
pub fn parse_result_json(s: &str) -> Result<(String, String), String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
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
    Ok((src, tgt))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Python interpreter of the engine venv. Override with LT_ENGINE_PYTHON.
fn engine_python() -> String {
    if let Ok(p) = std::env::var("LT_ENGINE_PYTHON") {
        return p;
    }
    let rel = if cfg!(windows) {
        ".venv-engine/Scripts/python.exe"
    } else {
        ".venv-engine/bin/python"
    };
    repo_root().join(rel).to_string_lossy().into_owned()
}

/// Directory containing the `lt_engine` package (repo `python/`), used as cwd so `-m lt_engine` resolves.
fn python_dir() -> PathBuf {
    repo_root().join("python")
}

/// (program, args) to run the modular engine, writing output into out_dir.
pub fn build_command(input: &Path, out_dir: &Path) -> (String, Vec<String>) {
    (
        engine_python(),
        vec![
            "-m".to_string(),
            "lt_engine".to_string(),
            "--file".to_string(),
            input.to_string_lossy().into_owned(),
            "--out-dir".to_string(),
            out_dir.to_string_lossy().into_owned(),
        ],
    )
}

/// Translates `input` to English audio + text via the lt_engine modular engine.
/// Writes output into a fresh temp dir and reads the produced wav + result.json.
pub fn translate_file(input: &Path) -> Result<TranslationOutput, String> {
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

    let (program, args) = build_command(input, &out_dir);
    let output = std::process::Command::new(&program)
        .args(&args)
        .current_dir(python_dir())
        .output()
        .map_err(|e| {
            format!("failed to start engine '{program}': {e}. Is the .venv-engine set up?")
        })?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&out_dir);
        return Err(format!(
            "translation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let wav = out_dir.join("output.wav");
    if !wav.exists() {
        let _ = std::fs::remove_dir_all(&out_dir);
        return Err("engine produced no output.wav".to_string());
    }

    let json = std::fs::read_to_string(out_dir.join("result.json"))
        .map_err(|e| format!("failed to read result.json: {e}"))?;
    let (source_text, translated_text) = parse_result_json(&json)?;

    Ok(TranslationOutput {
        output_wav: wav,
        source_text,
        translated_text,
    })
}

#[cfg(test)]
mod json_tests {
    use super::*;

    #[test]
    fn parses_both_texts() {
        let (s, t) =
            parse_result_json(r#"{"source_text":"hola","translated_text":"hello"}"#).unwrap();
        assert_eq!(s, "hola");
        assert_eq!(t, "hello");
    }

    #[test]
    fn missing_fields_default_empty() {
        let (s, t) = parse_result_json("{}").unwrap();
        assert_eq!(s, "");
        assert_eq!(t, "");
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_result_json("not json").is_err());
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn build_command_runs_lt_engine_module() {
        let (_program, args) = build_command(Path::new("in.wav"), Path::new("out"));
        assert_eq!(
            args,
            vec!["-m", "lt_engine", "--file", "in.wav", "--out-dir", "out"]
        );
    }
}
