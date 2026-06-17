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

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Python interpreter of the engine venv. Override with LT_ENGINE_PYTHON.
pub fn engine_python() -> String {
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
pub fn python_dir() -> PathBuf {
    repo_root().join("python")
}

/// Translates `input` to English audio + text via the persistent translation server.
pub fn translate_file(input: &Path) -> Result<TranslationOutput, String> {
    crate::translation::engine_server::translate(input)
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
